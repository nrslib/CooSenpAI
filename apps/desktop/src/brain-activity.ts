import type { JudgeDecision } from "./types.js";
export interface RateActivity {
  readonly schema: "rate-activity-v1";
  readonly input_id: string;
  readonly image_sha256: string;
  readonly graph_fingerprint: string;
  readonly total_neurons: number;
  readonly positioned_neurons: number;
  readonly total_steps: number;
  readonly hmax: number;
  readonly encoding: "f32-le-hex";
  readonly coordinate_source: string;
  readonly neurons: readonly { readonly body_id: string; readonly position: readonly [number, number, number] }[];
  readonly frames: readonly { readonly step: number; readonly values: string }[];
}
export interface BrainActivityModule {
  readonly moduleIndex: number;
  readonly status: "available" | "unavailable" | "invalid" | "failed";
  readonly unavailableReason: string | null;
  readonly activity: RateActivity | null;
  readonly evaluation: BrainModuleEvaluation | null;
  readonly holdReason: string | null;
  readonly error: string | null;
  readonly preview: BrainActivityPreview | null;
}
export interface BrainActivityObservation {
  readonly inputId: string;
  readonly occurredAt: string;
  readonly mode: "shadow" | "follow" | "pass-through" | null;
  readonly decision: JudgeDecision | null;
  readonly modules: readonly BrainActivityModule[];
}

export interface BrainModuleEvaluation {
  readonly novelty: number | null;
  readonly relevance: number | null;
  readonly action: JudgeDecision["action"];
  readonly readiness: string;
}
export interface BrainActivityPreview {
  readonly imageSha256: string;
  readonly width: number;
  readonly height: number;
  readonly pngHex: string;
}
export type BrainActivitySummary = Omit<BrainActivityObservation, "modules"> & {
  readonly modules: readonly Pick<BrainActivityModule, "moduleIndex" | "status" | "unavailableReason">[];
};
export interface BrainActivityEntry { readonly recordId: number; readonly observation: BrainActivitySummary }
export interface BrainActivityHistory {
  readonly generation: number;
  readonly revision: number;
  readonly latest: BrainActivitySummary | null;
  readonly entries: readonly BrainActivityEntry[];
}
export interface BrainActivityRecord { readonly generation: number; readonly recordId: number; readonly observation: BrainActivityObservation }
export interface BrainActivitySelection { readonly generation: number; readonly recordId: number; readonly inputId: string }

export function decodeActivityFrame(activity: RateActivity, index: number): Float32Array {
  const frame = activity.frames[index];
  if (frame === undefined || frame.values.length !== activity.neurons.length * 8) throw new Error("Invalid activity frame");
  const bytes = new Uint8Array(frame.values.length / 2);
  for (let offset = 0; offset < bytes.length; offset += 1) bytes[offset] = Number.parseInt(frame.values.slice(offset * 2, offset * 2 + 2), 16);
  const view = new DataView(bytes.buffer);
  return Float32Array.from({ length: activity.neurons.length }, (_, index) => view.getFloat32(index * 4, true));
}

export function activityIntensity(value: number, hmax: number): number {
  return Math.log1p(value / 1e-8) / Math.log1p(hmax / 1e-8);
}

export type ActivityDisplayMode = "activity" | "change";

export function activityFrameChange(activity: RateActivity, index: number): Float64Array | null {
  if (index === 0) return null;
  const current = decodeActivityFrame(activity, index);
  const previous = decodeActivityFrame(activity, index - 1);
  return Float64Array.from(current, (value, neuron) => value - previous[neuron]!);
}

export function activityChangeScale(activity: RateActivity): number {
  let peak = 0;
  for (let frame = 1; frame < activity.frames.length; frame += 1) {
    for (const difference of activityFrameChange(activity, frame)!) peak = Math.max(peak, Math.abs(difference));
  }
  return peak;
}

export function activityDisplayIntensities(activity: RateActivity, index: number, mode: ActivityDisplayMode, changeScale: number): Float32Array {
  if (mode === "activity") return decodeActivityFrame(activity, index).map((value) => activityIntensity(value, activity.hmax));
  const differences = activityFrameChange(activity, index);
  if (differences === null || changeScale === 0) return new Float32Array(activity.neurons.length);
  return Float32Array.from(differences, (value) => value / changeScale);
}
