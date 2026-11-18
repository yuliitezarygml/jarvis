#import <AVFoundation/AVFoundation.h>
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <stdatomic.h>
#include <stdint.h>
#include <string.h>

// All engine changes are serialized. The audio tap only touches a bounded
// capture buffer.
static AVAudioEngine *engine;
static AVAudioPlayerNode *player;
static AVAudioFormat *playFormat;
static NSMutableData *input;
static BOOL recording;
static int pending;
static uint64_t generation;
static NSObject *gate(void) {
  static NSObject *object;
  static dispatch_once_t once;
  dispatch_once(&once, ^{
    object = [NSObject new];
    input = [NSMutableData new];
  });
  return object;
}
static void clearPlayback(void) {
  generation++;
  pending = 0;
  [player stop];
}
static void teardown(void) {
  clearPlayback();
  if (recording)
    [engine.inputNode removeTapOnBus:0];
  recording = NO;
  [engine stop];
  player = nil;
  engine = nil;
  @synchronized(input) {
    [input setLength:0];
  }
}
static BOOL startEngine(BOOL mic, BOOL echo) {
  if (engine && engine.isRunning && (!mic || recording))
    return YES;
  teardown();
  @try {
    engine = [AVAudioEngine new];
    NSError *error = nil;
    if (mic && echo) {
      if (@available(macOS 10.15, *)) {
        if (![engine.inputNode setVoiceProcessingEnabled:YES error:&error]) {
          teardown();
          return NO;
        }
      }
    }
    player = [AVAudioPlayerNode new];
    [engine attachNode:player];
    playFormat = [[AVAudioFormat alloc] initStandardFormatWithSampleRate:24000
                                                                channels:1];
    [engine connect:player to:engine.mainMixerNode format:playFormat];
    if (mic) {
      AVAudioInputNode *node = engine.inputNode;
      AVAudioFormat *source = [node outputFormatForBus:0];
      if (!source.channelCount || source.sampleRate < 8000) {
        teardown();
        return NO;
      }
      // VoiceProcessingIO requires both client scopes at the capture sample
      // rate. The main mixer resamples the 24 kHz player into that hardware
      // clock.
      if (echo) {
        AVAudioFormat *duplex = [[AVAudioFormat alloc]
            initStandardFormatWithSampleRate:source.sampleRate
                                    channels:1];
        [engine connect:engine.mainMixerNode
                     to:engine.outputNode
                 format:duplex];
      }
      AVAudioFormat *target =
          [[AVAudioFormat alloc] initStandardFormatWithSampleRate:16000
                                                         channels:1];
      AVAudioConverter *converter =
          [[AVAudioConverter alloc] initFromFormat:source toFormat:target];
      if (!converter) {
        teardown();
        return NO;
      }
      [node installTapOnBus:0
                 bufferSize:1024
                     format:source
                      block:^(AVAudioPCMBuffer *buf, AVAudioTime *when) {
                        (void)when;
                        AVAudioFrameCount capacity =
                            (AVAudioFrameCount)(buf.frameLength * 16000.0 /
                                                source.sampleRate) +
                            64;
                        AVAudioPCMBuffer *out = [[AVAudioPCMBuffer alloc]
                            initWithPCMFormat:target
                                frameCapacity:capacity];
                        __block BOOL given = NO;
                        NSError *err = nil;
                        [converter
                               convertToBuffer:out
                                         error:&err
                            withInputFromBlock:^AVAudioBuffer *(
                                AVAudioPacketCount count,
                                AVAudioConverterInputStatus *status) {
                              (void)count;
                              *status =
                                  given ? AVAudioConverterInputStatus_NoDataNow
                                        : AVAudioConverterInputStatus_HaveData;
                              if (given)
                                return nil;
                              given = YES;
                              return buf;
                            }];
                        if (!err && out.frameLength)
                          @synchronized(input) {
                            if (input.length > 16000 * 3 * sizeof(float))
                              [input setLength:0];
                            [input appendBytes:out.floatChannelData[0]
                                        length:out.frameLength * sizeof(float)];
                          }
                      }];
      recording = YES;
    }
    [engine prepare];
    if (![engine startAndReturnError:&error]) {
      teardown();
      return NO;
    }
    return YES;
  } @catch (NSException *e) {
    teardown();
    return NO;
  }
}
int jv_mic_start(int continuous) {
  @autoreleasepool {
    AVAuthorizationStatus auth =
        [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
    if (auth == AVAuthorizationStatusNotDetermined) {
      dispatch_semaphore_t sem = dispatch_semaphore_create(0);
      [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio
                               completionHandler:^(BOOL ok) {
                                 (void)ok;
                                 dispatch_semaphore_signal(sem);
                               }];
      if (dispatch_semaphore_wait(
              sem, dispatch_time(DISPATCH_TIME_NOW, 60 * NSEC_PER_SEC)))
        return -2;
      auth = [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
    }
    if (auth != AVAuthorizationStatusAuthorized)
      return -1;
    @synchronized(gate()) {
      return startEngine(YES, continuous != 0) ? 1 : 0;
    }
  }
}
int jv_mic_status(void) {
  return (int)[AVCaptureDevice
      authorizationStatusForMediaType:AVMediaTypeAudio];
}
void jv_mic_end(void) {
  @autoreleasepool {
    @synchronized(gate()) {
      if (recording)
        teardown();
    }
  }
}
int jv_read(float *dst, int cap) {
  @autoreleasepool {
    @synchronized(input) {
      int n = MIN((NSUInteger)cap, input.length / sizeof(float));
      if (n) {
        memcpy(dst, input.bytes, n * sizeof(float));
        [input replaceBytesInRange:NSMakeRange(0, n * sizeof(float))
                         withBytes:NULL
                            length:0];
      }
      return n;
    }
  }
}
int jv_play(const int16_t *pcm, int n) {
  @autoreleasepool {
    @synchronized(gate()) {
      if (!startEngine(NO, NO))
        return -1;
      if (pending >= 5)
        return 0;
      AVAudioPCMBuffer *buf =
          [[AVAudioPCMBuffer alloc] initWithPCMFormat:playFormat
                                        frameCapacity:n];
      buf.frameLength = n;
      for (int i = 0; i < n; i++)
        buf.floatChannelData[0][i] = pcm[i] / 32768.0f;
      uint64_t id = generation;
      pending++;
      [player scheduleBuffer:buf
          completionCallbackType:AVAudioPlayerNodeCompletionDataPlayedBack
               completionHandler:^(AVAudioPlayerNodeCompletionCallbackType t) {
                 (void)t;
                 dispatch_async(
                     dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
                       @synchronized(gate()) {
                         if (generation == id && pending > 0)
                           pending--;
                       }
                     });
               }];
      if (!player.isPlaying)
        [player play];
      return 1;
    }
  }
}
void jv_stop(void) {
  @autoreleasepool {
    @synchronized(gate()) {
      clearPlayback();
    }
  }
}
int jv_pending(void) {
  @synchronized(gate()) {
    return pending;
  }
}
void jv_gpu(char *out, int cap) {
  @autoreleasepool {
    NSString *name = MTLCreateSystemDefaultDevice().name;
    if (name)
      [name getCString:out maxLength:cap encoding:NSUTF8StringEncoding];
  }
}
