#include "speaker_audio_ring.h"
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>

struct CoosenpaiAudioChanges {
    _Atomic uint64_t sequence;
    _Atomic uint64_t device_list_sequence;
    _Atomic uint64_t process_list_sequence;
};

struct CoosenpaiAudioRing {
    CoosenpaiAudioChanges *changes;
    _Atomic uint64_t sequence;
    uint32_t buffer_count;
    uint32_t channels_per_buffer;
    uint32_t bytes_per_frame;
    uint32_t tap_count;
    uint32_t *buffer_channels;
    uint32_t *tap_channels;
    uint32_t *channel_buffers;
    uint32_t *channel_indices;
    size_t *buffer_offsets;
    size_t slot_bytes;
    uint32_t frame_capacity;
    uint32_t slot_count;
    uint32_t *frames;
    uint64_t *host_times;
    unsigned char *storage;
    _Atomic uint64_t written;
    _Atomic uint64_t read;
    _Atomic uint64_t received;
    _Atomic uint64_t overflows;
    _Atomic uint64_t max_depth;
    _Atomic uint64_t max_callback_host_ticks;
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

uint64_t coosenpai_audio_device_list_sequence(const CoosenpaiAudioChanges *changes) {
    return atomic_load_explicit(&changes->device_list_sequence, memory_order_acquire);
}

uint64_t coosenpai_audio_process_list_sequence(const CoosenpaiAudioChanges *changes) {
    return atomic_load_explicit(&changes->process_list_sequence, memory_order_acquire);
}

OSStatus coosenpai_audio_property_changed(AudioObjectID object, UInt32 count,
    const AudioObjectPropertyAddress *addresses, void *context) {
    CoosenpaiAudioChanges *changes = context;
    bool device_list_only = object == kAudioObjectSystemObject && count > 0;
    bool process_list_only = device_list_only;
    for (UInt32 i = 0; i < count; ++i) {
        if (addresses[i].mSelector != kAudioHardwarePropertyDevices) device_list_only = false;
        if (addresses[i].mSelector != kAudioHardwarePropertyProcessObjectList) process_list_only = false;
    }
    if (device_list_only) {
        atomic_fetch_add_explicit(&changes->device_list_sequence, 1, memory_order_release);
    } else if (process_list_only) {
        atomic_fetch_add_explicit(&changes->process_list_sequence, 1, memory_order_release);
    } else {
        atomic_fetch_add_explicit(&changes->sequence, 1, memory_order_release);
    }
    return noErr;
}

CoosenpaiAudioRing *coosenpai_audio_ring_create(const AudioStreamBasicDescription *format,
    uint32_t frame_capacity, uint32_t slot_count, CoosenpaiAudioChanges *changes) {
    if (changes == NULL || frame_capacity == 0 || frame_capacity > 65536 || slot_count < 2 || slot_count > 8192
        || format->mFormatID != kAudioFormatLinearPCM || format->mBytesPerFrame == 0 || format->mBytesPerFrame > 128
        || format->mChannelsPerFrame == 0) return NULL;
    CoosenpaiAudioRing *ring = calloc(1, sizeof(*ring));
    if (ring == NULL) return NULL;
    ring->buffer_count = (format->mFormatFlags & kAudioFormatFlagIsNonInterleaved)
        ? format->mChannelsPerFrame : 1;
    ring->channels_per_buffer = format->mChannelsPerFrame / ring->buffer_count;
    ring->bytes_per_frame = format->mBytesPerFrame;
    ring->frame_capacity = frame_capacity;
    ring->slot_count = slot_count;
    ring->changes = changes;
    atomic_init(&ring->sequence, coosenpai_audio_changes_sequence(changes));
    if (frame_capacity > SIZE_MAX / ring->bytes_per_frame / ring->buffer_count) {
        free(ring);
        return NULL;
    }
    size_t bytes = (size_t)frame_capacity * ring->bytes_per_frame * ring->buffer_count;
    if (bytes > SIZE_MAX / slot_count) { free(ring); return NULL; }
    ring->slot_bytes = bytes;
    ring->frames = calloc(slot_count, sizeof(uint32_t));
    ring->host_times = calloc(slot_count, sizeof(uint64_t));
    ring->storage = calloc(slot_count, bytes);
    if (ring->frames == NULL || ring->host_times == NULL || ring->storage == NULL) {
        coosenpai_audio_ring_destroy(ring);
        return NULL;
    }
    atomic_init(&ring->written, 0);
    atomic_init(&ring->read, 0);
    atomic_init(&ring->received, 0);
    atomic_init(&ring->fault, COOSENPAI_AUDIO_RING_OK);
    return ring;
}

CoosenpaiAudioRing *coosenpai_audio_ring_create_tap_mix(
    const AudioStreamBasicDescription *input_format, const uint32_t *buffer_channels,
    uint32_t buffer_count, const uint32_t *tap_channels, uint32_t tap_count,
    uint32_t frame_capacity, uint32_t slot_count, CoosenpaiAudioChanges *changes) {
    if (input_format == NULL || buffer_channels == NULL || tap_channels == NULL || changes == NULL
        || input_format->mFormatID != kAudioFormatLinearPCM
        || (input_format->mFormatFlags & (kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked))
            != (kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked)
        || (input_format->mFormatFlags & kAudioFormatFlagIsBigEndian) != 0
        || input_format->mBitsPerChannel != 32 || input_format->mFramesPerPacket != 1
        || input_format->mChannelsPerFrame == 0
        || input_format->mBytesPerPacket != input_format->mBytesPerFrame
        || buffer_count == 0 || tap_count == 0 || frame_capacity == 0 || frame_capacity > 65536
        || slot_count < 2 || slot_count > 8192) return NULL;
    uint64_t expected_frame_bytes = input_format->mFormatFlags & kAudioFormatFlagIsNonInterleaved
        ? sizeof(float) : (uint64_t)input_format->mChannelsPerFrame * sizeof(float);
    if (input_format->mBytesPerFrame != expected_frame_bytes) return NULL;

    uint64_t input_channels = 0;
    uint64_t output_channels = 0;
    for (uint32_t i = 0; i < buffer_count; ++i) {
        if (buffer_channels[i] == 0) return NULL;
        input_channels += buffer_channels[i];
    }
    for (uint32_t i = 0; i < tap_count; ++i) {
        if (tap_channels[i] == 0) return NULL;
        output_channels += tap_channels[i];
    }
    if (input_channels != output_channels || input_channels > UINT32_MAX / sizeof(float) / frame_capacity
        || input_channels > SIZE_MAX / sizeof(float) / frame_capacity) return NULL;
    size_t slot_bytes = (size_t)input_channels * sizeof(float) * frame_capacity;
    if (slot_bytes > SIZE_MAX / slot_count) return NULL;

    CoosenpaiAudioRing *ring = calloc(1, sizeof(*ring));
    if (ring == NULL) return NULL;
    ring->buffer_channels = calloc(buffer_count, sizeof(uint32_t));
    ring->buffer_offsets = calloc(buffer_count, sizeof(size_t));
    ring->tap_channels = calloc(tap_count, sizeof(uint32_t));
    ring->channel_buffers = calloc((size_t)input_channels, sizeof(uint32_t));
    ring->channel_indices = calloc((size_t)input_channels, sizeof(uint32_t));
    ring->frames = calloc(slot_count, sizeof(uint32_t));
    ring->host_times = calloc(slot_count, sizeof(uint64_t));
    ring->storage = calloc(slot_count, slot_bytes);
    if (ring->buffer_channels == NULL || ring->buffer_offsets == NULL || ring->tap_channels == NULL
        || ring->channel_buffers == NULL || ring->channel_indices == NULL
        || ring->frames == NULL || ring->host_times == NULL || ring->storage == NULL) {
        coosenpai_audio_ring_destroy(ring);
        return NULL;
    }
    memcpy(ring->buffer_channels, buffer_channels, buffer_count * sizeof(uint32_t));
    memcpy(ring->tap_channels, tap_channels, tap_count * sizeof(uint32_t));
    size_t offset = 0;
    uint32_t channel = 0;
    for (uint32_t i = 0; i < buffer_count; ++i) {
        ring->buffer_offsets[i] = offset;
        for (uint32_t local = 0; local < buffer_channels[i]; ++local, ++channel) {
            ring->channel_buffers[channel] = i;
            ring->channel_indices[channel] = local;
        }
        offset += (size_t)buffer_channels[i] * sizeof(float) * frame_capacity;
    }
    ring->buffer_count = buffer_count;
    ring->tap_count = tap_count;
    ring->bytes_per_frame = sizeof(float);
    ring->frame_capacity = frame_capacity;
    ring->slot_count = slot_count;
    ring->slot_bytes = slot_bytes;
    ring->changes = changes;
    atomic_init(&ring->sequence, coosenpai_audio_changes_sequence(changes));
    atomic_init(&ring->written, 0);
    atomic_init(&ring->read, 0);
    atomic_init(&ring->received, 0);
    atomic_init(&ring->fault, COOSENPAI_AUDIO_RING_OK);
    return ring;
}

void coosenpai_audio_ring_destroy(CoosenpaiAudioRing *ring) {
    if (ring == NULL) return;
    free(ring->buffer_channels);
    free(ring->buffer_offsets);
    free(ring->tap_channels);
    free(ring->channel_buffers);
    free(ring->channel_indices);
    free(ring->storage);
    free(ring->frames);
    free(ring->host_times);
    free(ring);
}

uint32_t coosenpai_audio_ring_fault(const CoosenpaiAudioRing *ring) {
    return atomic_load_explicit(&ring->fault, memory_order_acquire);
}

uint64_t coosenpai_audio_ring_received(const CoosenpaiAudioRing *ring) {
    return atomic_load_explicit(&ring->received, memory_order_acquire);
}

uint64_t coosenpai_audio_ring_overflows(const CoosenpaiAudioRing *ring) {
    return atomic_load_explicit(&ring->overflows, memory_order_relaxed);
}

uint64_t coosenpai_audio_ring_max_depth(const CoosenpaiAudioRing *ring) {
    return atomic_load_explicit(&ring->max_depth, memory_order_relaxed);
}

uint64_t coosenpai_audio_ring_max_callback_host_ticks(const CoosenpaiAudioRing *ring) {
    return atomic_load_explicit(&ring->max_callback_host_ticks, memory_order_relaxed);
}

static void record_max(_Atomic uint64_t *target, uint64_t value) {
    uint64_t observed = atomic_load_explicit(target, memory_order_relaxed);
    while (observed < value && !atomic_compare_exchange_weak_explicit(target, &observed, value,
        memory_order_relaxed, memory_order_relaxed)) {}
}

void coosenpai_audio_ring_record_callback_host_ticks(CoosenpaiAudioRing *ring, uint64_t ticks) {
    record_max(&ring->max_callback_host_ticks, ticks);
}

void coosenpai_audio_ring_accept_configuration(CoosenpaiAudioRing *ring, uint64_t sequence) {
    atomic_store_explicit(&ring->sequence, sequence, memory_order_release);
}

static void coosenpai_audio_ring_push_buffers(CoosenpaiAudioRing *ring,
    const AudioBuffer *buffers, uint64_t host_time) {
    if (coosenpai_audio_changes_sequence(ring->changes) != atomic_load_explicit(&ring->sequence, memory_order_acquire)) return;
    uint32_t first_frame_bytes = ring->tap_count != 0
        ? ring->buffer_channels[0] * sizeof(float) : ring->bytes_per_frame;
    if (buffers[0].mDataByteSize % first_frame_bytes != 0) goto invalid;
    uint32_t frames = buffers[0].mDataByteSize / first_frame_bytes;
    if (frames == 0) return;
    if (host_time == 0) {
        atomic_store_explicit(&ring->fault, COOSENPAI_AUDIO_RING_INVALID_TIMESTAMP, memory_order_release);
        return;
    }
    if (frames > ring->frame_capacity) goto invalid;
    for (uint32_t i = 0; i < ring->buffer_count; ++i) {
        const AudioBuffer *buffer = &buffers[i];
        uint32_t channels = ring->tap_count != 0 ? ring->buffer_channels[i] : ring->channels_per_buffer;
        uint32_t frame_bytes = ring->tap_count != 0 ? channels * sizeof(float) : ring->bytes_per_frame;
        if (buffer->mData == NULL || buffer->mNumberChannels != channels
            || buffer->mDataByteSize != frames * frame_bytes) goto invalid;
    }
    atomic_fetch_add_explicit(&ring->received, 1, memory_order_relaxed);
    uint64_t written = atomic_load_explicit(&ring->written, memory_order_relaxed);
    uint64_t read = atomic_load_explicit(&ring->read, memory_order_acquire);
    if (written - read == ring->slot_count) {
        atomic_fetch_add_explicit(&ring->overflows, 1, memory_order_relaxed);
        atomic_store_explicit(&ring->fault, COOSENPAI_AUDIO_RING_OVERFLOW, memory_order_release);
        return;
    }
    size_t slot = written % ring->slot_count;
    for (uint32_t i = 0; i < ring->buffer_count; ++i) {
        size_t buffer_bytes = (size_t)ring->frame_capacity * (ring->tap_count != 0
            ? ring->buffer_channels[i] * sizeof(float) : ring->bytes_per_frame);
        size_t offset = ring->tap_count != 0 ? ring->buffer_offsets[i] : i * buffer_bytes;
        memcpy(ring->storage + slot * ring->slot_bytes + offset,
            buffers[i].mData, (size_t)frames * (ring->tap_count != 0
                ? ring->buffer_channels[i] * sizeof(float) : ring->bytes_per_frame));
    }
    ring->frames[slot] = frames;
    ring->host_times[slot] = host_time;
    atomic_store_explicit(&ring->written, written + 1, memory_order_release);
    record_max(&ring->max_depth, written + 1 - read);
    return;
invalid:
    atomic_store_explicit(&ring->fault, COOSENPAI_AUDIO_RING_INVALID_LAYOUT, memory_order_release);
}

void coosenpai_audio_ring_push(CoosenpaiAudioRing *ring, const AudioBufferList *input, uint64_t host_time) {
    if (coosenpai_audio_changes_sequence(ring->changes) != atomic_load_explicit(&ring->sequence, memory_order_acquire)) return;
    if (input->mNumberBuffers == 0) return;
    if (input->mNumberBuffers != ring->buffer_count) goto invalid;
    coosenpai_audio_ring_push_buffers(ring, input->mBuffers, host_time);
    return;
invalid:
    atomic_store_explicit(&ring->fault, COOSENPAI_AUDIO_RING_INVALID_LAYOUT, memory_order_release);
}

uint32_t coosenpai_audio_ring_pop(CoosenpaiAudioRing *ring, AudioBufferList *output, uint64_t *host_time) {
    if (coosenpai_audio_changes_sequence(ring->changes) != atomic_load_explicit(&ring->sequence, memory_order_acquire)) return 0;
    uint64_t read = atomic_load_explicit(&ring->read, memory_order_relaxed);
    if (read == atomic_load_explicit(&ring->written, memory_order_acquire)) return 0;
    size_t slot = read % ring->slot_count;
    uint32_t frames = ring->frames[slot];
    uint32_t output_buffers = ring->tap_count != 0 ? ring->tap_count : ring->buffer_count;
    if (output->mNumberBuffers != output_buffers) return 0;
    for (uint32_t i = 0; i < output_buffers; ++i) {
        if (output->mBuffers[i].mData == NULL
            || output->mBuffers[i].mDataByteSize < frames * ring->bytes_per_frame
            || (ring->tap_count != 0 && output->mBuffers[i].mNumberChannels != 1)) return 0;
    }
    *host_time = ring->host_times[slot];
    if (ring->tap_count == 0) {
        size_t buffer_bytes = (size_t)ring->frame_capacity * ring->bytes_per_frame;
        for (uint32_t i = 0; i < ring->buffer_count; ++i) {
            memcpy(output->mBuffers[i].mData,
                ring->storage + slot * ring->slot_bytes + i * buffer_bytes,
                frames * ring->bytes_per_frame);
            output->mBuffers[i].mDataByteSize = frames * ring->bytes_per_frame;
        }
    } else {
        uint32_t global_channel = 0;
        for (uint32_t tap = 0; tap < ring->tap_count; ++tap) {
            float *destination = output->mBuffers[tap].mData;
            for (uint32_t frame = 0; frame < frames; ++frame) {
                float sum = 0;
                uint32_t channel = global_channel;
                for (uint32_t tap_channel = 0; tap_channel < ring->tap_channels[tap]; ++tap_channel, ++channel) {
                    uint32_t buffer_index = ring->channel_buffers[channel];
                    uint32_t local_channel = ring->channel_indices[channel];
                    const float *source = (const float *)(ring->storage + slot * ring->slot_bytes
                        + ring->buffer_offsets[buffer_index]);
                    sum += source[(size_t)frame * ring->buffer_channels[buffer_index] + local_channel];
                }
                destination[frame] = sum / ring->tap_channels[tap];
            }
            global_channel += ring->tap_channels[tap];
            output->mBuffers[tap].mDataByteSize = frames * sizeof(float);
        }
    }
    atomic_store_explicit(&ring->read, read + 1, memory_order_release);
    return frames;
}
