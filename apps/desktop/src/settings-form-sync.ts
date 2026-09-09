import type { FormState } from "./settings-form.js";
import { fieldValues, formFields, type FormFields } from "./settings-form-fields.js";
import type { CooSenpaiConfig } from "./types.js";

interface FormInput {
  readonly fields: FormFields;
  readonly avatarImage: readonly number[] | null;
  readonly avatarFileName: string | null;
}
interface FormBasis extends FormInput {
  readonly configRevision: number;
  readonly generation: number;
}
export interface SettingsDraft extends FormInput {
  readonly revision: number;
  readonly basis: FormBasis;
}
export interface SettingsReflection {
  readonly config: CooSenpaiConfig;
  readonly fields: FormFields;
  readonly configRevision: number;
  readonly generation: number;
  readonly revision: number;
  readonly avatarImage: readonly number[] | null;
  readonly avatarFileName: string | null;
  readonly avatarImageLoadFailed: boolean;
}
function formInput(form: FormState): FormInput {
  return { fields: formFields(form), avatarImage: form.avatarImage ?? null, avatarFileName: form.avatarFileName ?? null };
}

// 同期編集と、実際に表示へ採用した反映の配送番号だけを保持する。再マージは Rust が行う。
export class SettingsFormSync {
  form: FormState;
  private revision = 0;
  private basis: FormBasis;

  constructor(form: FormState, configRevision: number) {
    this.form = form;
    this.basis = { ...formInput(form), configRevision, generation: 0 };
  }
  edit(form: FormState): void {
    this.form = form;
    this.revision += 1;
  }
  input(): SettingsDraft {
    return { ...formInput(this.form), revision: this.revision, basis: this.basis };
  }
  reflect(value: SettingsReflection): boolean {
    if (value.revision !== this.revision || value.generation <= this.basis.generation || value.configRevision < this.basis.configRevision) return false;
    this.form = { ...fieldValues(value.fields), avatarImageLoadFailed: value.avatarImageLoadFailed, avatarImage: value.avatarImage ?? undefined, avatarFileName: value.avatarFileName ?? undefined };
    this.basis = { ...formInput(this.form), configRevision: value.configRevision, generation: value.generation };
    return true;
  }
}
