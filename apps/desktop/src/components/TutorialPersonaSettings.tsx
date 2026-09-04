import type { ComponentProps, ReactElement } from "react";

import { GeneralSettings } from "./GeneralSettings.js";
import { ProviderSettings } from "./ProviderSettings.js";

interface Props {
  readonly general: ComponentProps<typeof GeneralSettings>;
  readonly provider: ComponentProps<typeof ProviderSettings>;
}

export function TutorialPersonaSettings({ general, provider }: Props): ReactElement {
  return <><GeneralSettings {...general} /><ProviderSettings {...provider} /></>;
}
