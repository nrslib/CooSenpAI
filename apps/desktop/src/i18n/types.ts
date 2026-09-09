export type LocalizedShape<T> = {
  readonly [K in keyof T]: T[K] extends string
    ? string
    : T[K] extends Record<string, unknown>
      ? LocalizedShape<T[K]>
      : never;
};
