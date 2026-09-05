import type { AppSnapshot } from "../types.js";
import type { FormState, SettingsErrorFor, SettingsUpdate } from "../settings-form.js";

export interface SettingsCategoryProps {
  readonly form: FormState;
  readonly snapshot: AppSnapshot;
  readonly saving: boolean;
  readonly update: SettingsUpdate;
  readonly errorFor: SettingsErrorFor;
}
