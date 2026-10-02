#ifndef COOSENPAI_SPEAKER_AUDIO_RING_H
#define COOSENPAI_SPEAKER_AUDIO_RING_H

#include <CoreAudio/CoreAudio.h>
#include <stdbool.h>
#include <stdint.h>

CF_ASSUME_NONNULL_BEGIN

typedef struct CoosenpaiAudioChanges CoosenpaiAudioChanges;
typedef struct CoosenpaiAudioRing CoosenpaiAudioRing;

CoosenpaiAudioChanges * _Nullable coosenpai_audio_changes_create(void);
void coosenpai_audio_changes_destroy(CoosenpaiAudioChanges *changes);
uint64_t coosenpai_audio_changes_sequence(const CoosenpaiAudioChanges *changes);
uint64_t coosenpai_audio_device_list_sequence(const CoosenpaiAudioChanges *changes);
OSStatus coosenpai_audio_property_changed(AudioObjectID object, UInt32 count,
    const AudioObjectPropertyAddress * _Nonnull addresses, void * _Nullable context);

enum {
    COOSENPAI_AUDIO_RING_OK = 0,
    COOSENPAI_AUDIO_RING_OVERFLOW = 1,
    COOSENPAI_AUDIO_RING_INVALID_LAYOUT = 2,
    COOSENPAI_AUDIO_RING_INVALID_TIMESTAMP = 3
};

CoosenpaiAudioRing * _Nullable coosenpai_audio_ring_create(const AudioStreamBasicDescription *format,
    uint32_t frame_capacity, uint32_t slot_count, CoosenpaiAudioChanges *changes);
void coosenpai_audio_ring_destroy(CoosenpaiAudioRing * _Nullable ring);
uint32_t coosenpai_audio_ring_fault(const CoosenpaiAudioRing *ring);
uint64_t coosenpai_audio_ring_received(const CoosenpaiAudioRing *ring);
void coosenpai_audio_ring_push(CoosenpaiAudioRing *ring, const AudioBufferList *input, uint64_t host_time);
void coosenpai_audio_ring_push_tap_suffix(CoosenpaiAudioRing *ring, const AudioBufferList *input,
    uint64_t host_time, uint32_t preceding_channels);
uint32_t coosenpai_audio_ring_pop(CoosenpaiAudioRing *ring, AudioBufferList *output, uint64_t *host_time);
CF_ASSUME_NONNULL_END

#endif
