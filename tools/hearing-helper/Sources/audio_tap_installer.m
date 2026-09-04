#import "audio_tap_installer.h"

BOOL coosenpai_install_audio_tap(
    AVAudioInputNode *input_node,
    AVAudioFrameCount buffer_size,
    AVAudioFormat *format,
    AVAudioNodeTapBlock block,
    NSError **error
) {
    @try {
        [input_node installTapOnBus:0
                          bufferSize:buffer_size
                              format:format
                               block:block];
        return YES;
    } @catch (NSException *exception) {
        if (error != NULL) {
            *error = [NSError errorWithDomain:@"coosenpai.hearing.audio-tap"
                                         code:1
                                     userInfo:@{
                                         NSLocalizedDescriptionKey: exception.reason
                                             ?: exception.name
                                     }];
        }
        return NO;
    }
}
