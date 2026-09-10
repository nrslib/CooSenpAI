#include "speaker_audio_ring.h"
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>

struct CoosenpaiAudioChanges {
    _Atomic uint64_t sequence;
};

struct CoosenpaiAudioRing {
    CoosenpaiAudioChanges *changes;
    uint64_t sequence;
    uint32_t buffer_count;
    uint32_t channels_per_buffer;
    uint32_t bytes_per_frame;
    uint32_t frame_capacity;
    uint32_t slot_count;
    uint32_t *frames;
    unsigned char *storage;
    _Atomic uint64_t written;
    _Atomic uint64_t read;
    _Atomic uint64_t received;
    _Atomic uint32_t fault;
};

_Static_assert(ATOMIC_LONG_LOCK_FREE == 2 && ATOMIC_LLONG_LOCK_FREE == 2 && ATOMIC_INT_LOCK_FREE == 2,
    "Audio callbacks require lock-free counters");

CoosenpaiAudioChanges *coosenpai_audio_changes_create(void) {
    CoosenpaiAudioChanges *changes = calloc(1, sizeof(*changes));
    if (changes != NULL) atomic_init(&changes->sequence, 0);
    return changes;
}

void coosenpai_audio_changes_destroy(CoosenpaiAudioChanges *changes) { free(changes); }

uint64_t coosenpai_audio_changes_sequence(const CoosenpaiAudioChanges *changes) {
    return atomic_load_explicit(&changes->sequence, memory_order_acquire);
}

OSStatus coosenpai_audio_property_changed(AudioObjectID object, UInt32 count,
    const AudioObjectPropertyAddress *addresses, void *context) {
    CoosenpaiAudioChanges *changes = context;
    atomic_fetch_add_explicit(&changes->sequence, 1, memory_order_release);
    return noErr;
}

CoosenpaiAudioRing *coosenpai_audio_ring_create(const AudioStreamBasicDescription *format,
    uint32_t frame_capacity, uint32_t slot_count, CoosenpaiAudioChanges *changes) {
    if (changes == NULL || frame_capacity == 0 || frame_capacity > 65536 || slot_count < 2 || slot_count > 1024
        || format->mFormatID != kAudioFormatLinearPCM || format->mBytesPerFrame == 0 || format->mBytesPerFrame > 16
        || format->mChannelsPerFrame == 0 || format->mChannelsPerFrame > 2) return NULL;
    CoosenpaiAudioRing *ring = calloc(1, sizeof(*ring));
    if (ring == NULL) return NULL;
    ring->buffer_count = (format->mFormatFlags & kAudioFormatFlagIsNonInterleaved)
        ? format->mChannelsPerFrame : 1;
    ring->channels_per_buffer = format->mChannelsPerFrame / ring->buffer_count;
    ring->bytes_per_frame = format->mBytesPerFrame;
    ring->frame_capacity = frame_capacity;
    ring->slot_count = slot_count;
    ring->changes = changes;
    ring->sequence = coosenpai_audio_changes_sequence(changes);
    size_t bytes = (size_t)frame_capacity * ring->bytes_per_frame * ring->buffer_count;
    if (bytes > SIZE_MAX / slot_count) { free(ring); return NULL; }
    ring->frames = calloc(slot_count, sizeof(uint32_t));
    ring->storage = calloc(slot_count, bytes);
    if (ring->frames == NULL || ring->storage == NULL) {
        coosenpai_audio_ring_destroy(ring);
        return NULL;
    }
    atomic_init(&ring->written, 0);
    atomic_init(&ring->read, 0);
    atomic_init(&ring->received, 0);
    atomic_init(&ring->fault, COOSENPAI_AUDIO_RING_OK);
    return ring;
}

void coosenpai_audio_ring_destroy(CoosenpaiAudioRing *ring) {
    if (ring == NULL) return;
    free(ring->storage);
    free(ring->frames);
    free(ring);
}

uint32_t coosenpai_audio_ring_fault(const CoosenpaiAudioRing *ring) {
    return atomic_load_explicit(&ring->fault, memory_order_acquire);
}

uint64_t coosenpai_audio_ring_received(const CoosenpaiAudioRing *ring) {
    return atomic_load_explicit(&ring->received, memory_order_acquire);
}

void coosenpai_audio_ring_push(CoosenpaiAudioRing *ring, const AudioBufferList *input) {
    if (coosenpai_audio_changes_sequence(ring->changes) != ring->sequence) return;
    if (input->mNumberBuffers == 0) return;
    if (input->mNumberBuffers != ring->buffer_count) goto invalid;
    uint32_t frames = input->mBuffers[0].mDataByteSize / ring->bytes_per_frame;
    if (frames == 0) return;
    if (frames > ring->frame_capacity) goto invalid;
    for (uint32_t i = 0; i < ring->buffer_count; ++i) {
        const AudioBuffer *buffer = &input->mBuffers[i];
        if (buffer->mData == NULL || buffer->mNumberChannels != ring->channels_per_buffer
            || buffer->mDataByteSize != frames * ring->bytes_per_frame) goto invalid;
    }
    atomic_fetch_add_explicit(&ring->received, 1, memory_order_relaxed);
    uint64_t written = atomic_load_explicit(&ring->written, memory_order_relaxed);
    uint64_t read = atomic_load_explicit(&ring->read, memory_order_acquire);
    if (written - read == ring->slot_count) {
        atomic_store_explicit(&ring->fault, COOSENPAI_AUDIO_RING_OVERFLOW, memory_order_release);
        return;
    }
    size_t buffer_bytes = (size_t)ring->frame_capacity * ring->bytes_per_frame;
    size_t slot = written % ring->slot_count;
    for (uint32_t i = 0; i < ring->buffer_count; ++i) {
        memcpy(ring->storage + (slot * ring->buffer_count + i) * buffer_bytes,
            input->mBuffers[i].mData, frames * ring->bytes_per_frame);
    }
    ring->frames[slot] = frames;
    atomic_store_explicit(&ring->written, written + 1, memory_order_release);
    return;
invalid:
    atomic_store_explicit(&ring->fault, COOSENPAI_AUDIO_RING_INVALID_LAYOUT, memory_order_release);
}

uint32_t coosenpai_audio_ring_pop(CoosenpaiAudioRing *ring, AudioBufferList *output) {
    if (coosenpai_audio_changes_sequence(ring->changes) != ring->sequence) return 0;
    uint64_t read = atomic_load_explicit(&ring->read, memory_order_relaxed);
    if (read == atomic_load_explicit(&ring->written, memory_order_acquire)) return 0;
    size_t slot = read % ring->slot_count;
    uint32_t frames = ring->frames[slot];
    if (output->mNumberBuffers != ring->buffer_count) return 0;
    for (uint32_t i = 0; i < ring->buffer_count; ++i) {
        if (output->mBuffers[i].mData == NULL
            || output->mBuffers[i].mDataByteSize < frames * ring->bytes_per_frame) return 0;
    }
    size_t buffer_bytes = (size_t)ring->frame_capacity * ring->bytes_per_frame;
    for (uint32_t i = 0; i < ring->buffer_count; ++i) {
        memcpy(output->mBuffers[i].mData,
            ring->storage + (slot * ring->buffer_count + i) * buffer_bytes,
            frames * ring->bytes_per_frame);
        output->mBuffers[i].mDataByteSize = frames * ring->bytes_per_frame;
    }
    atomic_store_explicit(&ring->read, read + 1, memory_order_release);
    return frames;
}

OSStatus coosenpai_audio_ring_io_proc(AudioObjectID device, const AudioTimeStamp *now,
    const AudioBufferList *input, const AudioTimeStamp *input_time,
    AudioBufferList *output, const AudioTimeStamp *output_time, void *context) {
    coosenpai_audio_ring_push(context, input);
    return noErr;
}
