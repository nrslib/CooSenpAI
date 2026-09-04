import type { ReactElement } from "react";

import type { SettingsCategoryProps } from "./SettingsCategoryProps.js";

interface Props extends SettingsCategoryProps {
  readonly onRestartTutorial: () => void;
  readonly onRestartSetup: () => void;
  readonly onResetConversation: () => void;
}

export function SetupSettings({ snapshot, onRestartTutorial, onRestartSetup, onResetConversation }: Props): ReactElement {
  return <fieldset id="settings-setup"><legend>セットアップ</legend>
    <div className="button-row"><button type="button" onClick={onRestartTutorial}>チュートリアルをもう一度</button><button type="button" onClick={onRestartSetup}>セットアップをやり直す</button></div>
    <button type="button" disabled={snapshot.onboarding.tutorialActive === true} onClick={onResetConversation}>会話をリセット</button>
  </fieldset>;
}
