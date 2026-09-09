export interface VoiceOutputSnapshot {
  readonly revision: number;
  readonly speaking: boolean;
  readonly message: string | null;
}

export interface VoiceOutputVoice {
  readonly id: number;
  readonly name: string;
  readonly styleName: string;
}
