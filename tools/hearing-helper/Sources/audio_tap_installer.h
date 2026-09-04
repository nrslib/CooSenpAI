#import <AVFAudio/AVFAudio.h>
#import <Foundation/Foundation.h>

FOUNDATION_EXPORT BOOL coosenpai_install_audio_tap(
    AVAudioInputNode *input_node,
    AVAudioFrameCount buffer_size,
    AVAudioFormat *format,
    AVAudioNodeTapBlock block,
    NSError **error
);
