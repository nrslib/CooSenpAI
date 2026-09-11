#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    Ja,
    En,
}

impl Locale {
    pub fn from_config(value: &str) -> Self {
        match value {
            "en" => Self::En,
            _ => Self::Ja,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ja => "ja",
            Self::En => "en",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextKey {
    ResponseLanguage,
    CommonYes,
    CommonNo,
    CommonRetry,
    CommonOpenSettings,
    ConversationResetPrompt,
    ConversationResetComplete,
    TutorialComplete,
    TutorialSkip,
    TutorialNotStarted,
    TutorialGuideNotReady,
    ShortcutUnset,
    ShortcutSpace,
    EmptyClipboard,
    CopyCompleted,
    UpdateAvailable,
    UpdateOperationInProgress,
    UpdateShuttingDown,
    UpdateCheckFailed,
    UpdateDebugBuild,
    UpdatePendingMissing,
    UpdateDownloadVerifyFailed,
    UpdatePreviousPreserved,
    UpdateApplyFailed,
    UpdateProcessFailed,
    UpdateConfigMissing,
    UpdateDisabled,
    UpdateConfigInvalid,
    UpdateDestinationInvalid,
    UpdateTransportInitFailed,
    UpdateRedirectNotAllowed,
    UpdateMetadataTooLarge,
    UpdateMetadataInvalid,
    UpdateSystemVersionUnavailable,
    UpdateBundleMinimumInvalid,
    UpdateBundleMinimumMismatch,
    UpdateArtifactMissing,
    UpdateArtifactUrlInvalid,
    UpdateDestinationNotAllowed,
    UpdateServerCommunication,
    UpdateHttpStatus,
    UpdateFileTooLarge,
    UpdateDownloadInterrupted,
    UpdatePublicKeyInvalid,
    UpdateSignatureInvalid,
    UpdateSignatureTooLarge,
    UpdateSignatureEncodingInvalid,
    UpdateSignatureUtf8Invalid,
    UpdateFileSizeInvalid,
    UpdateSignatureVerificationFailed,
    UpdateVersionInvalid,
    UpdateWorkdirFailed,
    UpdateArchiveInvalid,
    UpdateEntryLimit,
    UpdateEntryDuplicate,
    UpdateSpecialFile,
    UpdateExpandedSizeLimit,
    UpdateDirectoryData,
    UpdateEntryLengthMismatch,
    UpdateEntryPathTooLong,
    UpdatePathOutsideBundle,
    UpdateInfoPlistMissing,
    UpdateInfoPlistInvalid,
    UpdateInfoPlistParseFailed,
    UpdateInfoPlistNotDictionary,
    UpdateBundleIdentityMismatch,
    UpdateBundleVersionMissing,
    UpdateBundleVersionInvalid,
    UpdateBundleVersionMismatch,
    UpdateExecutableMissing,
    UpdateExecutableInvalid,
    UpdateArchitectureUnsupported,
    UpdateMachHeaderReadFailed,
    UpdateMachFormatInvalid,
    UpdateArchitectureMismatch,
    UpdateExpandedTotalLimit,
    UpdateAppLocationInvalid,
    UpdateAppExecutableLocationInvalid,
    UpdateAppMustBeInApplications,
    UpdateAppLocationOpenFailed,
    UpdateInstallLocationUnsafe,
    UpdateTargetNotDirectory,
    UpdateTargetChanged,
    UpdateVerifyArchiveTooLarge,
    UpdateVerifyUsage,
    UpdateVerifySucceeded,
    UpdateVerifyFailed,
    ModelCatalogLoadFailed,
    ModelCatalogSaveFailed,
    WatchConfigInvalid,
    WatchGenerationMissing,
    SetupWaiting,
    StartupTutorialStateReadFailed,
    StartupConversationGenerationReadFailed,
    StartupConversationRecoveryPrepareFailed,
    StartupConversationRecoveryFailed,
    StartupConversationInitializationFailed,
    StartupConfigUnsupportedVersion,
    WindowOpen,
    WindowWatchStart,
    WindowWatchStop,
    WindowSettings,
    WindowResetConversation,
    WindowQuit,
    WindowShortcuts,
    ShortcutSendText,
    ShortcutCaptureRegion,
    ShortcutMicrophone,
    ShortcutTogglePanel,
    ShortcutToggleAvatar,
    ShortcutToggleWatch,
    ShortcutCopyLastReply,
    TrayRecording,
    TrayWatching,
    TrayPaused,
    WindowStateUnavailable,
    MainWindowMissing,
    MainWindowShowFailed,
    MainWindowFocusFailed,
    FactoryExecutableDirectoryUnavailable,
    FactoryApiKeyCheckFailed,
    FactoryApiKeyReadFailed,
    FactoryApiKeySaveFailed,
    FactoryApiKeyDeleteFailed,
    FactoryAuthApplyFailed,
    FactoryAuthRestoreFailed,
    FactoryBridgeMissing,
    FactoryNodeMissing,
    FactoryBridgeInitFailed,
    FactoryTutorialMissing,
    FactoryEnglishTutorialMissing,
    FactoryShutdown,
    FactoryModelInfoMissing,
    FactoryConnectionEmpty,
    FactoryProviderInvalid,
    FactoryPersonaLoadFailed,
    FactoryBuiltinPersonaMissing,
    WatchConfigFix,
    WatchPermissionRequired,
    WatchStartFailed,
    WatchOperationFailed,
    WatchFullscreenConsent,
    WatchFullscreenDetail,
    WatchTargetMissing,
    WatchFullscreenTarget,
    CaptureDispositionAccepted,
    CaptureDispositionUnchanged,
    CaptureDispositionSuppressed,
    CaptureDispositionSelfApplication,
    CaptureDispositionOwnBoundsUnavailable,
    CaptureDispositionWindowUnavailable,
    CaptureDispositionMinSpacing,
    FactRememberPrompt,
    FactCandidateMissing,
    FactCandidateExpired,
    CaptureSendInProgress,
    CaptureSelectedTextFailed,
    CaptureSelectedTextCopyFailed,
    CaptureClipboardReadFailed,
    CaptureImageReadFailed,
    CaptureNoRegion,
    CaptureImageLoadFailed,
    CaptureRegionMismatch,
    CaptureTextUnavailable,
    CaptureOperationFailed,
    CaptureAccessibilityRequired,
    CaptureShortcutDisabled,
    CaptureAccessibilitySettingsFailed,
    CaptureMessageTooLong,
    ShortcutInvalid,
    ShortcutDuplicate,
    ShortcutRegisterFailed,
    ShortcutRestoreFailed,
    SpeechSourceInvalid,
    SpeechTextEmpty,
    SpeechTextTooLong,
    SpeechConfirmationMissing,
    SpeechStaleInput,
    SpeechRecognitionSettingsFailed,
    SpeechMicrophoneSettingsFailed,
    SpeechSettingsKindInvalid,
    SpeechSending,
    SpeechEnding,
    SpeechGenerationMissing,
    SpeechStartFailed,
    SpeechInputEmpty,
    SpeechHelperMissing,
    SpeechNoSpeech,
    SpeechGenericFailure,
    SpeechMicrophoneDenied,
    SpeechMicrophoneRestricted,
    SpeechMicrophoneUnavailable,
    SpeechRecognitionDenied,
    SpeechRecognitionRestricted,
    SpeechRecognitionUnavailable,
    SpeechLocaleUnavailable,
    SpeechOnDeviceUnsupported,
    SpeechInputDeviceUnavailable,
    SpeechInputDeviceListFailed,
    SpeechInputDeviceListFallback,
    SpeechKeyStateFailed,
    SpeechInputDeviceFallback,
    SpeechInputDeviceFallbackShort,
    ScreenPermissionAllow,
    ScreenPermissionRestart,
    ScreenPermissionRestricted,
    ScreenPermissionUnavailable,
    AudioSourceRequired,
    AudioScreenPermissionRequired,
    AudioSystemPermissionRequired,
    AudioSystemFailed,
    AudioSystemDeviceUnavailable,
    AudioSystemFormatFailed,
    AudioSystemOverflow,
    AudioSystemStartupTimeout,

    AudioHelperMissing,
    AudioHelperUnexpectedExit,
    AudioObservationSaveFailed,
    AudioWorkerStopped,
    AudioWarningGeneric,
    SetupSelectionEmpty,
    SetupSavedResponseUnavailable,
    SetupResponsePending,
    SetupProviderInvalid,
    SetupApiKeyEmpty,
    SetupLoginApiKeyRejected,
    SetupConfigUnavailable,
    SettingsNumericRequired,
    SettingsUnsignedBuildOnly,
    AttachmentReadFailed,
    ProcessNoFrontmostApplication,
    ConfigInvalidObject,
    ConfigInvalidArray,
    ConfigUnknownKey,
    ConfigRequired,
    ConfigPositiveInteger,
    ConfigNonnegativeInteger,
    ConfigOptionalNonnegativeInteger,
    ConfigPositiveNumber,
    ConfigNonemptyString,
    ConfigNonemptyOrNullString,
    ConfigColor,
    ConfigAvatarPath,
    ConfigWhitespaceString,
    ConfigAllowedValues,
    ConfigEither,
    ConfigPersonaId,
    ConfigExecutable,
    ConfigRange,
    ConfigPositiveRange,
    ConfigActiveThreshold,
    ConfigSpacing,
    ConfigDuplicate,
    ConfigBundleId,
    ConfigAppName,
    ConfigShortcut,
    ConfigTauriShortcut,
    ConfigEscapeShortcut,
    ConfigName,
    ConfigReviewTime,
    ConfigReminderLimit,
    ConfigReminderId,
    ConfigReminderTime,
    ConfigReminderTheme,
    ConfigLanguage,
    ConfigVoiceOutputProvider,
    ConfigInvalidValue,
    ConfigFileRead,
    ConfigJson,
    ConfigValidation,
    ConfigVersionInteger,
    ConfigUnsupportedVersion,
    ConfigRevisionConflict,
    ConfigLock,
    ConfigCandidateBuildFailed,
    SetupBubbleDisplayFailed,
    SetupLanguageIntro,
    SetupLanguageJa,
    SetupLanguageEn,
    SetupLanguageNext,
    SetupLanguageRequired,
    SetupLanguageInvalid,
    SetupLanguageSelectionUnavailable,
    SetupProviderSelectionUnavailable,
    SetupConnectionMethodSelectionUnavailable,
    SetupResponseUnavailable,
    SetupExpired,
    TutorialCurrentStepMissing,
    TutorialAutoAdvance,
    SetupProviderRequired,
    SetupConnectionMethodRequired,
    SetupApiKeyRequired,
    SetupApiKeyInvalid,
    SetupApiKeyNotAccepted,
    InvalidBubbleAction,
    SetupConnectionTimeout,
    SetupUnavailable,
    SetupNext,
    SetupApiKeyLabel,
    SetupApiKeyPlaceholder,
    SetupConnectionConfirm,
    SetupApiKeySubmit,
    SetupLoginCodex,
    SetupLoginClaude,
    SetupLoginCli,
    SetupSettings,
    SetupRetry,
    SetupNoneDetail,
    TutorialCancelled,
    TutorialObserverActivity,
    TutorialManualDismissNotAllowed,
    TutorialSettingsPresentationUnavailable,
    CommandShuttingDown,
    CommandTransitionInProgress,
    CommandTutorialFinishing,
    CommandSetupRequired,
    CommandTutorialOperationNotAllowed,
    CommandStaleGeneration,
    CommandRuntimeUnavailable,
    CommandInvalidInput,
    CommandWindowNotAllowed,
    CommandResultUnavailable,
    IdentifierEmpty,
    MessageEmpty,
    MessageTooLong,
    ModelPopupOpenFailed,
    ModelPopupCloseFailed,
    DetailsOpenFailed,
    DetailsDataflowLogReadFailed,
    DetailsDataflowPathOpenFailed,
    ModelConfigObject,
    ModelConfigCompanionOnly,
    ModelConfigCompanionObject,
    ModelConfigRequired,
    ModelConfigKeys,
    AvatarImageProcessFailed,
    AvatarImageProcessingIncomplete,
    AvatarImageStageFailed,
    AvatarImageStageIncomplete,
    AvatarWindowMissing,
    AvatarPositionFailed,
    AvatarVisibilityFailed,
    ConversationGenerationNotFound,
    ConversationGenerationOperationFailed,
    ConversationDataReadWriteFailed,
    ConversationDataInvalid,
    ConversationDataLocked,
    ConversationSwitchRecoveryFailed,
    AssertivenessInvalid,
    PersonaNotFound,
    PersonaChangedNotice,
    SystemSettingsOpenFailed,
    LicenseDocumentOpenFailed,
    TutorialPersonaWaiting,
    BubbleAppearancePreviewInvalid,
    BubbleRendererAttemptInvalid,
    BubbleGenerationUnknown,
    BubbleHeightOutOfRange,
    BubbleResizeFailed,
    MemoryPeriodInvalid,
    MemoryOperationFailed,
    PersonaOperationFailed,
    RunningApplicationsListFailed,
    CopyLastReplyEmpty,
    CopyLastReplyFailed,
    LaunchAtLoginSyncFailed,
    RuntimeClosed,
    RuntimeConfigUpdateCancelled,
    RuntimeObservationCancelled,
    RuntimeObserverUnavailable,
    RuntimeCompanionUnavailable,
    RuntimeResponseDropped,
    RuntimeProviderStartsBlocked,
    RuntimeStaleWatchScope,
    RuntimeObserverError,
    RuntimeCompanionError,
    RuntimeConfigInvalid,
    ConfigUpdateFailed,
    RuntimeFactoryError,
    RuntimeCancelableReplyMissing,
    RuntimeRetryableReplyMissing,
    RuntimeMemoryMaintenanceUnavailable,
    RuntimeFactoryRebuildUnavailable,
    VoiceOutputDisabled,
    VoiceOutputTextTooLong,
    VoiceOutputPending,
    VoiceOutputInvalidInput,
    VoiceOutputFailed,
    VoiceOutputTestText,
    VoicevoxEngineFailed,
    VoicevoxInvalidResponse,
    VoicevoxFileFailed,
    VoicevoxListCancelled,
    VoicevoxInvalidStyle,
    VoicevoxPlaybackFailed,
    VoicevoxSelectVoice,
    VoicevoxUnsupportedProvider,
    VoicevoxUnsupportedLink,
    VoicevoxOpenLinkFailed,
    ConfigVoicevoxStyleRange,
}

pub fn text(key: TextKey, locale: Locale) -> &'static str {
    match (key, locale) {
        (TextKey::ResponseLanguage, Locale::Ja) => "",
        (TextKey::ResponseLanguage, Locale::En) => {
            "Respond in English when the UI language is en. Keep the response concise and natural."
        }
        (TextKey::CommonYes, Locale::Ja) => "はい",
        (TextKey::CommonYes, Locale::En) => "Yes",
        (TextKey::CommonNo, Locale::Ja) => "いいえ",
        (TextKey::CommonNo, Locale::En) => "No",
        (TextKey::CommonRetry, Locale::Ja) => "もう一度試す",
        (TextKey::CommonRetry, Locale::En) => "Try again",
        (TextKey::CommonOpenSettings, Locale::Ja) => "設定を開く",
        (TextKey::CommonOpenSettings, Locale::En) => "Open settings",
        (TextKey::ConversationResetPrompt, Locale::Ja) => {
            "今の会話を閉じて新しく始めますか？（履歴は残ります）"
        }
        (TextKey::ConversationResetPrompt, Locale::En) => {
            "Close this conversation and start a new one? (History will be kept.)"
        }
        (TextKey::ConversationResetComplete, Locale::Ja) => "会話をリセットしました",
        (TextKey::ConversationResetComplete, Locale::En) => "The conversation was reset.",
        (TextKey::TutorialComplete, Locale::Ja) => {
            "チュートリアルを終わりました。ここからは本番です"
        }
        (TextKey::TutorialComplete, Locale::En) => {
            "The tutorial is complete. This is where the real work begins."
        }
        (TextKey::TutorialSkip, Locale::Ja) => "この項目をスキップ",
        (TextKey::TutorialSkip, Locale::En) => "Skip this step",
        (TextKey::TutorialNotStarted, Locale::Ja) => "チュートリアルが開始されていません",
        (TextKey::TutorialNotStarted, Locale::En) => "The tutorial has not started.",
        (TextKey::TutorialGuideNotReady, Locale::Ja) => {
            "チュートリアル案内の表示確認が完了していません"
        }
        (TextKey::TutorialGuideNotReady, Locale::En) => {
            "The tutorial guide is not ready to be confirmed."
        }
        (TextKey::ShortcutUnset, Locale::Ja) => "未設定",
        (TextKey::ShortcutUnset, Locale::En) => "Not set",
        (TextKey::ShortcutSpace, Locale::Ja) => "空白",
        (TextKey::ShortcutSpace, Locale::En) => "Space",
        (TextKey::EmptyClipboard, Locale::Ja) => "文章を選んで {shortcut} を押してください",
        (TextKey::EmptyClipboard, Locale::En) => "Select text, then press {shortcut}.",
        (TextKey::CopyCompleted, Locale::Ja) => "コピーしました",
        (TextKey::CopyCompleted, Locale::En) => "Copied.",
        (TextKey::UpdateAvailable, Locale::Ja) => {
            "新しいバージョン v{version} があります。アプリを開いて「更新する」を押してください"
        }
        (TextKey::UpdateAvailable, Locale::En) => {
            "A new version v{version} is available. Open the app and choose “Update”."
        }
        (TextKey::UpdateOperationInProgress, Locale::Ja) => "更新処理を実行中です",
        (TextKey::UpdateOperationInProgress, Locale::En) => "An update is already in progress.",
        (TextKey::UpdateShuttingDown, Locale::Ja) => "アプリを終了しています",
        (TextKey::UpdateShuttingDown, Locale::En) => "The app is shutting down.",
        (TextKey::UpdateCheckFailed, Locale::Ja) => "更新を確認できませんでした: {error}",
        (TextKey::UpdateCheckFailed, Locale::En) => "Could not check for updates: {error}",
        (TextKey::UpdateDebugBuild, Locale::Ja) => {
            "開発版では更新を適用できません。配布版を使用してください"
        }
        (TextKey::UpdateDebugBuild, Locale::En) => {
            "Updates cannot be applied to development builds. Use a distributed build."
        }
        (TextKey::UpdatePendingMissing, Locale::Ja) => "更新を確認してから実行してください",
        (TextKey::UpdatePendingMissing, Locale::En) => "Check for an update before applying it.",
        (TextKey::UpdateDownloadVerifyFailed, Locale::Ja) => {
            "更新ファイルを取得・検証できませんでした: {error}"
        }
        (TextKey::UpdateDownloadVerifyFailed, Locale::En) => {
            "Could not download or verify the update: {error}"
        }
        (TextKey::UpdatePreviousPreserved, Locale::Ja) => {
            "旧版を保持して更新を中止しました: {error}"
        }
        (TextKey::UpdatePreviousPreserved, Locale::En) => {
            "The update was cancelled while preserving the previous version: {error}"
        }
        (TextKey::UpdateApplyFailed, Locale::Ja) => "更新を適用できませんでした: {error}",
        (TextKey::UpdateApplyFailed, Locale::En) => "Could not apply the update: {error}",
        (TextKey::UpdateProcessFailed, Locale::Ja) => "更新処理に失敗しました: {error}",
        (TextKey::UpdateProcessFailed, Locale::En) => "The update failed: {error}",
        (TextKey::UpdateConfigMissing, Locale::Ja) => "更新設定がありません",
        (TextKey::UpdateConfigMissing, Locale::En) => "The update configuration is missing.",
        (TextKey::UpdateDisabled, Locale::Ja) => {
            "設定の「更新の確認」を有効にして反映してください"
        }
        (TextKey::UpdateDisabled, Locale::En) => {
            "Enable “Check for updates” in Settings and apply the change."
        }
        (TextKey::UpdateConfigInvalid, Locale::Ja) => "更新設定が不正です",
        (TextKey::UpdateConfigInvalid, Locale::En) => "The update configuration is invalid.",
        (TextKey::UpdateDestinationInvalid, Locale::Ja) => {
            "更新先または architecture が不正です"
        }
        (TextKey::UpdateDestinationInvalid, Locale::En) => {
            "The update destination or architecture is invalid."
        }
        (TextKey::UpdateTransportInitFailed, Locale::Ja) => "更新用の通信を初期化できません",
        (TextKey::UpdateTransportInitFailed, Locale::En) => {
            "Could not initialize update networking."
        }
        (TextKey::UpdateRedirectNotAllowed, Locale::Ja) => "更新のリダイレクト先を許可できません",
        (TextKey::UpdateRedirectNotAllowed, Locale::En) => {
            "The update redirect destination is not allowed."
        }
        (TextKey::UpdateMetadataTooLarge, Locale::Ja) => "更新情報のサイズが上限を超えています",
        (TextKey::UpdateMetadataTooLarge, Locale::En) => "The update metadata is too large.",
        (TextKey::UpdateMetadataInvalid, Locale::Ja) => "更新情報の形式が不正です",
        (TextKey::UpdateMetadataInvalid, Locale::En) => "The update metadata is invalid.",
        (TextKey::UpdateSystemVersionUnavailable, Locale::Ja) => "実行中の macOS の版を取得できません",
        (TextKey::UpdateSystemVersionUnavailable, Locale::En) => "Could not determine the running macOS version.",
        (TextKey::UpdateBundleMinimumInvalid, Locale::Ja) => "署名済みアプリの最低 macOS 版が欠落しているか不正です",
        (TextKey::UpdateBundleMinimumInvalid, Locale::En) => "The signed app's minimum macOS version is missing or invalid.",
        (TextKey::UpdateBundleMinimumMismatch, Locale::Ja) => "署名済みアプリの最低 macOS 版が更新情報と一致しません",
        (TextKey::UpdateBundleMinimumMismatch, Locale::En) => "The signed app's minimum macOS version does not match the update metadata.",
        (TextKey::UpdateArtifactMissing, Locale::Ja) => {
            "この architecture 用の更新ファイルがありません"
        }
        (TextKey::UpdateArtifactMissing, Locale::En) => {
            "There is no update artifact for this architecture."
        }
        (TextKey::UpdateArtifactUrlInvalid, Locale::Ja) => "更新ファイルの取得先が不正です",
        (TextKey::UpdateArtifactUrlInvalid, Locale::En) => {
            "The update artifact URL is invalid."
        }
        (TextKey::UpdateDestinationNotAllowed, Locale::Ja) => "更新ファイルの取得先を許可できません",
        (TextKey::UpdateDestinationNotAllowed, Locale::En) => {
            "The update download destination is not allowed."
        }
        (TextKey::UpdateServerCommunication, Locale::Ja) => "更新サーバーと通信できませんでした",
        (TextKey::UpdateServerCommunication, Locale::En) => {
            "Could not communicate with the update server."
        }
        (TextKey::UpdateHttpStatus, Locale::Ja) => "更新サーバーが HTTP {status} を返しました",
        (TextKey::UpdateHttpStatus, Locale::En) => {
            "The update server returned HTTP {status}."
        }
        (TextKey::UpdateFileTooLarge, Locale::Ja) => "更新ファイルのサイズが上限を超えています",
        (TextKey::UpdateFileTooLarge, Locale::En) => "The update file is too large.",
        (TextKey::UpdateDownloadInterrupted, Locale::Ja) => "更新ファイルの取得が中断されました",
        (TextKey::UpdateDownloadInterrupted, Locale::En) => "The update download was interrupted.",
        (TextKey::UpdatePublicKeyInvalid, Locale::Ja) => "更新公開鍵が不正です",
        (TextKey::UpdatePublicKeyInvalid, Locale::En) => "The update public key is invalid.",
        (TextKey::UpdateSignatureInvalid, Locale::Ja) => "更新署名が不正です",
        (TextKey::UpdateSignatureInvalid, Locale::En) => "The update signature is invalid.",
        (TextKey::UpdateSignatureTooLarge, Locale::Ja) => "更新署名のサイズが上限を超えています",
        (TextKey::UpdateSignatureTooLarge, Locale::En) => "The update signature is too large.",
        (TextKey::UpdateSignatureEncodingInvalid, Locale::Ja) => {
            "更新署名のエンコードが不正です"
        }
        (TextKey::UpdateSignatureEncodingInvalid, Locale::En) => {
            "The update signature encoding is invalid."
        }
        (TextKey::UpdateSignatureUtf8Invalid, Locale::Ja) => "更新署名が UTF-8 ではありません",
        (TextKey::UpdateSignatureUtf8Invalid, Locale::En) => {
            "The update signature is not UTF-8."
        }
        (TextKey::UpdateFileSizeInvalid, Locale::Ja) => "更新ファイルのサイズが不正です",
        (TextKey::UpdateFileSizeInvalid, Locale::En) => "The update file size is invalid.",
        (TextKey::UpdateSignatureVerificationFailed, Locale::Ja) => {
            "更新ファイルの署名検証に失敗しました"
        }
        (TextKey::UpdateSignatureVerificationFailed, Locale::En) => {
            "The update file signature could not be verified."
        }
        (TextKey::UpdateVersionInvalid, Locale::Ja) => {
            "更新先の版は現在より新しい stable 版である必要があります"
        }
        (TextKey::UpdateVersionInvalid, Locale::En) => {
            "The update must be a newer stable version."
        }
        (TextKey::UpdateWorkdirFailed, Locale::Ja) => {
            "更新の作業場所を作成できません。アプリの配置先への書き込み権限を確認してください"
        }
        (TextKey::UpdateWorkdirFailed, Locale::En) => {
            "Could not create a workspace for the update. Check write permission for the app location."
        }
        (TextKey::UpdateArchiveInvalid, Locale::Ja) => "更新アーカイブが不正です: {error}",
        (TextKey::UpdateArchiveInvalid, Locale::En) => "The update archive is invalid: {error}",
        (TextKey::UpdateEntryLimit, Locale::Ja) => "entry 数が上限を超えています",
        (TextKey::UpdateEntryLimit, Locale::En) => "The archive contains too many entries.",
        (TextKey::UpdateEntryDuplicate, Locale::Ja) => "entry が重複しています",
        (TextKey::UpdateEntryDuplicate, Locale::En) => "The archive contains duplicate entries.",
        (TextKey::UpdateSpecialFile, Locale::Ja) => "リンク・特殊ファイルは許可しません",
        (TextKey::UpdateSpecialFile, Locale::En) => "Links and special files are not allowed.",
        (TextKey::UpdateExpandedSizeLimit, Locale::Ja) => "展開サイズが上限を超えています",
        (TextKey::UpdateExpandedSizeLimit, Locale::En) => "The expanded archive is too large.",
        (TextKey::UpdateDirectoryData, Locale::Ja) => "ディレクトリ entry にデータがあります",
        (TextKey::UpdateDirectoryData, Locale::En) => "A directory entry contains data.",
        (TextKey::UpdateEntryLengthMismatch, Locale::Ja) => "entry の長さが一致しません",
        (TextKey::UpdateEntryLengthMismatch, Locale::En) => "An archive entry has an unexpected length.",
        (TextKey::UpdateEntryPathTooLong, Locale::Ja) => "entry のパスが長すぎます",
        (TextKey::UpdateEntryPathTooLong, Locale::En) => "An archive entry path is too long.",
        (TextKey::UpdatePathOutsideBundle, Locale::Ja) => "entry がアプリの外を参照しています",
        (TextKey::UpdatePathOutsideBundle, Locale::En) => "An archive entry points outside the app bundle.",
        (TextKey::UpdateInfoPlistMissing, Locale::Ja) => "更新アプリに Info.plist がありません",
        (TextKey::UpdateInfoPlistMissing, Locale::En) => "The updated app has no Info.plist.",
        (TextKey::UpdateInfoPlistInvalid, Locale::Ja) => "更新アプリの Info.plist が不正です",
        (TextKey::UpdateInfoPlistInvalid, Locale::En) => "The updated app's Info.plist is invalid.",
        (TextKey::UpdateInfoPlistParseFailed, Locale::Ja) => "更新アプリの Info.plist を解析できません",
        (TextKey::UpdateInfoPlistParseFailed, Locale::En) => "Could not parse the updated app's Info.plist.",
        (TextKey::UpdateInfoPlistNotDictionary, Locale::Ja) => {
            "更新アプリの Info.plist が辞書ではありません"
        }
        (TextKey::UpdateInfoPlistNotDictionary, Locale::En) => {
            "The updated app's Info.plist is not a dictionary."
        }
        (TextKey::UpdateBundleIdentityMismatch, Locale::Ja) => {
            "更新アプリの識別子または実行ファイル名が一致しません"
        }
        (TextKey::UpdateBundleIdentityMismatch, Locale::En) => {
            "The updated app identifier or executable name does not match."
        }
        (TextKey::UpdateBundleVersionMissing, Locale::Ja) => "更新アプリに版がありません",
        (TextKey::UpdateBundleVersionMissing, Locale::En) => "The updated app has no version.",
        (TextKey::UpdateBundleVersionInvalid, Locale::Ja) => "更新アプリの版が不正です",
        (TextKey::UpdateBundleVersionInvalid, Locale::En) => "The updated app version is invalid.",
        (TextKey::UpdateBundleVersionMismatch, Locale::Ja) => {
            "署名済みアプリの版が更新情報と一致しません"
        }
        (TextKey::UpdateBundleVersionMismatch, Locale::En) => {
            "The signed app version does not match the update metadata."
        }
        (TextKey::UpdateExecutableMissing, Locale::Ja) => "更新アプリに実行ファイルがありません",
        (TextKey::UpdateExecutableMissing, Locale::En) => "The updated app has no executable.",
        (TextKey::UpdateExecutableInvalid, Locale::Ja) => "更新アプリの実行ファイルが不正です",
        (TextKey::UpdateExecutableInvalid, Locale::En) => "The updated app executable is invalid.",
        (TextKey::UpdateArchitectureUnsupported, Locale::Ja) => "更新対象の architecture は非対応です",
        (TextKey::UpdateArchitectureUnsupported, Locale::En) => {
            "The update architecture is not supported."
        }
        (TextKey::UpdateMachHeaderReadFailed, Locale::Ja) => {
            "更新アプリの Mach-O ヘッダーを読み取れません"
        }
        (TextKey::UpdateMachHeaderReadFailed, Locale::En) => {
            "Could not read the updated app's Mach-O header."
        }
        (TextKey::UpdateMachFormatInvalid, Locale::Ja) => {
            "更新アプリは thin Mach-O 64-bit 実行ファイルである必要があります"
        }
        (TextKey::UpdateMachFormatInvalid, Locale::En) => {
            "The updated app must be a thin 64-bit Mach-O executable."
        }
        (TextKey::UpdateArchitectureMismatch, Locale::Ja) => {
            "署名済みアプリの architecture が更新対象と一致しません"
        }
        (TextKey::UpdateArchitectureMismatch, Locale::En) => {
            "The signed app architecture does not match the update target."
        }
        (TextKey::UpdateExpandedTotalLimit, Locale::Ja) => "展開総量が上限を超えています",
        (TextKey::UpdateExpandedTotalLimit, Locale::En) => "The total expanded size is too large.",
        (TextKey::UpdateAppLocationInvalid, Locale::Ja) => "アプリの配置先が不正です",
        (TextKey::UpdateAppLocationInvalid, Locale::En) => "The app location is invalid.",
        (TextKey::UpdateAppExecutableLocationInvalid, Locale::Ja) => {
            "アプリの実行ファイルの配置が不正です"
        }
        (TextKey::UpdateAppExecutableLocationInvalid, Locale::En) => {
            "The app executable is in an invalid location."
        }
        (TextKey::UpdateAppMustBeInApplications, Locale::Ja) => {
            "CooSenpAI.app を Applications にコピーして起動してから更新してください"
        }
        (TextKey::UpdateAppMustBeInApplications, Locale::En) => {
            "Copy CooSenpAI.app to Applications, launch it there, and then update."
        }
        (TextKey::UpdateAppLocationOpenFailed, Locale::Ja) => "アプリの配置先を開けません",
        (TextKey::UpdateAppLocationOpenFailed, Locale::En) => "Could not open the app location.",
        (TextKey::UpdateInstallLocationUnsafe, Locale::Ja) => {
            "アプリの配置先の権限が安全ではありません"
        }
        (TextKey::UpdateInstallLocationUnsafe, Locale::En) => {
            "The app location permissions are not safe."
        }
        (TextKey::UpdateTargetNotDirectory, Locale::Ja) => "更新対象がディレクトリではありません",
        (TextKey::UpdateTargetNotDirectory, Locale::En) => "The update target is not a directory.",
        (TextKey::UpdateTargetChanged, Locale::Ja) => "更新対象が処理中に変更されました",
        (TextKey::UpdateTargetChanged, Locale::En) => "The update target changed during the operation.",
        (TextKey::UpdateVerifyArchiveTooLarge, Locale::Ja) => "更新ファイルが上限を超えています",
        (TextKey::UpdateVerifyArchiveTooLarge, Locale::En) => "The update file exceeds the size limit.",
        (TextKey::UpdateVerifyUsage, Locale::Ja) => "使い方: coosenpai-update-verify ARCHIVE VERSION",
        (TextKey::UpdateVerifyUsage, Locale::En) => "Usage: coosenpai-update-verify ARCHIVE VERSION",
        (TextKey::UpdateVerifySucceeded, Locale::Ja) => {
            "更新署名・アーカイブ構造・アプリ識別子・版の検証が完了しました"
        }
        (TextKey::UpdateVerifySucceeded, Locale::En) => {
            "Update signature, archive structure, app identity, and version verified."
        }
        (TextKey::UpdateVerifyFailed, Locale::Ja) => "更新ファイルの検証に失敗しました: {error}",
        (TextKey::UpdateVerifyFailed, Locale::En) => "Update file verification failed: {error}",
        (TextKey::ModelCatalogLoadFailed, Locale::Ja) => "一覧を取得できませんでした",
        (TextKey::ModelCatalogLoadFailed, Locale::En) => "Could not load the model list.",
        (TextKey::ModelCatalogSaveFailed, Locale::Ja) => "一覧を保存できませんでした",
        (TextKey::ModelCatalogSaveFailed, Locale::En) => "Could not save the model list.",
        (TextKey::WatchConfigInvalid, Locale::Ja) => "設定を修正して保存してください",
        (TextKey::WatchConfigInvalid, Locale::En) => "Fix the settings and save them before trying again.",
        (TextKey::WatchGenerationMissing, Locale::Ja) => "見守り開始世代がありません",
        (TextKey::WatchGenerationMissing, Locale::En) => "There is no watch-start generation.",
        (TextKey::SetupWaiting, Locale::Ja) => "初回セットアップを待っています。",
        (TextKey::SetupWaiting, Locale::En) => "Waiting for initial setup.",
        (TextKey::StartupTutorialStateReadFailed, Locale::Ja) => {
            "初回設定の状態を読み取れません: {error}"
        }
        (TextKey::StartupTutorialStateReadFailed, Locale::En) => {
            "Could not read the initial setup state: {error}"
        }
        (TextKey::StartupConversationGenerationReadFailed, Locale::Ja) => {
            "会話の世代を読み取れません: {error}"
        }
        (TextKey::StartupConversationGenerationReadFailed, Locale::En) => {
            "Could not read the conversation generation: {error}"
        }
        (TextKey::StartupConversationRecoveryPrepareFailed, Locale::Ja) => {
            "起動時の会話復旧を準備できません: {error}"
        }
        (TextKey::StartupConversationRecoveryPrepareFailed, Locale::En) => {
            "Could not prepare conversation recovery at startup: {error}"
        }
        (TextKey::StartupConversationRecoveryFailed, Locale::Ja) => {
            "起動時の会話復旧に失敗しました: {error}"
        }
        (TextKey::StartupConversationRecoveryFailed, Locale::En) => {
            "Conversation recovery failed at startup: {error}"
        }
        (TextKey::StartupConversationInitializationFailed, Locale::Ja) => {
            "起動時に会話を初期化できません: {error}"
        }
        (TextKey::StartupConversationInitializationFailed, Locale::En) => {
            "Could not initialize the conversation at startup: {error}"
        }
        (TextKey::ConversationGenerationNotFound, Locale::Ja) => {
            "選択する会話世代が存在しません: {generation}"
        }
        (TextKey::ConversationGenerationNotFound, Locale::En) => {
            "The selected conversation generation does not exist: {generation}"
        }
        (TextKey::ConversationGenerationOperationFailed, Locale::Ja) => {
            "会話世代を更新できません: {error}"
        }
        (TextKey::ConversationGenerationOperationFailed, Locale::En) => {
            "Could not update the conversation generation: {error}"
        }
        (TextKey::ConversationDataReadWriteFailed, Locale::Ja) => {
            "会話データを読み書きできませんでした"
        }
        (TextKey::ConversationDataReadWriteFailed, Locale::En) => {
            "Could not read or write the conversation data."
        }
        (TextKey::ConversationDataInvalid, Locale::Ja) => "会話データが不正です",
        (TextKey::ConversationDataInvalid, Locale::En) => "The conversation data is invalid.",
        (TextKey::ConversationDataLocked, Locale::Ja) => {
            "別のプロセスが会話データを使用しています"
        }
        (TextKey::ConversationDataLocked, Locale::En) => {
            "Another process is using the conversation data."
        }
        (TextKey::ConversationSwitchRecoveryFailed, Locale::Ja) => {
            "会話世代の切り替えに失敗しました: {error}; 元の世代の復旧にも失敗しました: {recovery_error}"
        }
        (TextKey::ConversationSwitchRecoveryFailed, Locale::En) => {
            "Could not switch conversation generation: {error}; could not restore the original generation: {recovery_error}"
        }
        (TextKey::StartupConfigUnsupportedVersion, Locale::Ja) => {
            "設定バージョン {version} は未対応です。"
        }
        (TextKey::StartupConfigUnsupportedVersion, Locale::En) => {
            "Configuration version {version} is not supported."
        }
        (TextKey::WindowOpen, Locale::Ja) => "CooSenpAI を開く",
        (TextKey::WindowOpen, Locale::En) => "Open CooSenpAI",
        (TextKey::WindowWatchStart, Locale::Ja) => "見る",
        (TextKey::WindowWatchStart, Locale::En) => "Monitor screen",
        (TextKey::WindowWatchStop, Locale::Ja) => "休憩する",
        (TextKey::WindowWatchStop, Locale::En) => "Take a break",
        (TextKey::WindowSettings, Locale::Ja) => "設定",
        (TextKey::WindowSettings, Locale::En) => "Settings",
        (TextKey::WindowResetConversation, Locale::Ja) => "会話をリセット",
        (TextKey::WindowResetConversation, Locale::En) => "Reset conversation",
        (TextKey::WindowQuit, Locale::Ja) => "終了",
        (TextKey::WindowQuit, Locale::En) => "Quit",
        (TextKey::WindowShortcuts, Locale::Ja) => "ショートカット",
        (TextKey::WindowShortcuts, Locale::En) => "Shortcuts",
        (TextKey::ShortcutSendText, Locale::Ja) => "文章を渡す",
        (TextKey::ShortcutSendText, Locale::En) => "Send text",
        (TextKey::ShortcutCaptureRegion, Locale::Ja) => "画面を渡す",
        (TextKey::ShortcutCaptureRegion, Locale::En) => "Send screen",
        (TextKey::ShortcutMicrophone, Locale::Ja) => "声で話す",
        (TextKey::ShortcutMicrophone, Locale::En) => "Speak",
        (TextKey::ShortcutTogglePanel, Locale::Ja) => "パネルを開く",
        (TextKey::ShortcutTogglePanel, Locale::En) => "Open panel",
        (TextKey::ShortcutToggleAvatar, Locale::Ja) => "アバターを表示 / 非表示",
        (TextKey::ShortcutToggleAvatar, Locale::En) => "Show / hide avatar",
        (TextKey::ShortcutToggleWatch, Locale::Ja) => "見る / 休憩する",
        (TextKey::ShortcutToggleWatch, Locale::En) => "Monitor / take a break",
        (TextKey::ShortcutCopyLastReply, Locale::Ja) => "直近の返事をコピー",
        (TextKey::ShortcutCopyLastReply, Locale::En) => "Copy latest reply",
        (TextKey::TrayRecording, Locale::Ja) => "録音中",
        (TextKey::TrayRecording, Locale::En) => "Recording",
        (TextKey::TrayWatching, Locale::Ja) => "見ています",
        (TextKey::TrayWatching, Locale::En) => "Monitoring",
        (TextKey::TrayPaused, Locale::Ja) => "休憩中",
        (TextKey::TrayPaused, Locale::En) => "Paused",
        (TextKey::WindowStateUnavailable, Locale::Ja) => "desktop state がありません",
        (TextKey::WindowStateUnavailable, Locale::En) => "The desktop state is unavailable.",
        (TextKey::MainWindowMissing, Locale::Ja) => "メインウィンドウがありません",
        (TextKey::MainWindowMissing, Locale::En) => "The main window is unavailable.",
        (TextKey::MainWindowShowFailed, Locale::Ja) => "メインウィンドウを表示できません: {error}",
        (TextKey::MainWindowShowFailed, Locale::En) => "Could not show the main window: {error}",
        (TextKey::MainWindowFocusFailed, Locale::Ja) => "メインウィンドウを前面にできません: {error}",
        (TextKey::MainWindowFocusFailed, Locale::En) => "Could not focus the main window: {error}",
        (TextKey::FactoryExecutableDirectoryUnavailable, Locale::Ja) => {
            "実行ファイルのディレクトリを取得できません"
        }
        (TextKey::FactoryExecutableDirectoryUnavailable, Locale::En) => {
            "Could not get the executable directory."
        }
        (TextKey::FactoryApiKeyCheckFailed, Locale::Ja) => "API キーを確認できません",
        (TextKey::FactoryApiKeyCheckFailed, Locale::En) => "Could not check the API key.",
        (TextKey::FactoryApiKeyReadFailed, Locale::Ja) => "API キーを読み込めません",
        (TextKey::FactoryApiKeyReadFailed, Locale::En) => "Could not read the API key.",
        (TextKey::FactoryApiKeySaveFailed, Locale::Ja) => "API キーを保存できません",
        (TextKey::FactoryApiKeySaveFailed, Locale::En) => "Could not save the API key.",
        (TextKey::FactoryApiKeyDeleteFailed, Locale::Ja) => "API キーを削除できません",
        (TextKey::FactoryApiKeyDeleteFailed, Locale::En) => "Could not delete the API key.",
        (TextKey::FactoryAuthApplyFailed, Locale::Ja) => {
            "provider の認証設定を反映できません"
        }
        (TextKey::FactoryAuthApplyFailed, Locale::En) => {
            "Could not apply the provider authentication settings."
        }
        (TextKey::FactoryAuthRestoreFailed, Locale::Ja) => {
            "provider の認証設定を復元できません"
        }
        (TextKey::FactoryAuthRestoreFailed, Locale::En) => {
            "Could not restore the provider authentication settings."
        }
        (TextKey::FactoryBridgeMissing, Locale::Ja) => "provider bridge が見つかりません",
        (TextKey::FactoryBridgeMissing, Locale::En) => "The provider bridge was not found.",
        (TextKey::FactoryNodeMissing, Locale::Ja) => "Node.js 18 以上が見つかりません",
        (TextKey::FactoryNodeMissing, Locale::En) => "Node.js 18 or later was not found.",
        (TextKey::FactoryBridgeInitFailed, Locale::Ja) => "provider bridge を初期化できません",
        (TextKey::FactoryBridgeInitFailed, Locale::En) => "Could not initialize the provider bridge.",
        (TextKey::FactoryTutorialMissing, Locale::Ja) => "チュートリアル台本が見つかりません",
        (TextKey::FactoryTutorialMissing, Locale::En) => "The tutorial script was not found.",
        (TextKey::FactoryEnglishTutorialMissing, Locale::Ja) => {
            "英語のチュートリアル台本が見つかりません"
        }
        (TextKey::FactoryEnglishTutorialMissing, Locale::En) => {
            "The English tutorial script was not found."
        }
        (TextKey::FactoryShutdown, Locale::Ja) => "終了処理中です",
        (TextKey::FactoryShutdown, Locale::En) => "The app is shutting down.",
        (TextKey::FactoryModelInfoMissing, Locale::Ja) => "provider のモデル情報がありません",
        (TextKey::FactoryModelInfoMissing, Locale::En) => "The provider has no model information.",
        (TextKey::FactoryConnectionEmpty, Locale::Ja) => "接続確認の応答が空でした",
        (TextKey::FactoryConnectionEmpty, Locale::En) => "The connection check returned an empty response.",
        (TextKey::FactoryProviderInvalid, Locale::Ja) => "provider が不正です: {name}",
        (TextKey::FactoryProviderInvalid, Locale::En) => "The provider is invalid: {name}",
        (TextKey::FactoryPersonaLoadFailed, Locale::Ja) => "persona を読み込めません: {detail}",
        (TextKey::FactoryPersonaLoadFailed, Locale::En) => "Could not load the persona: {detail}",
        (TextKey::FactoryBuiltinPersonaMissing, Locale::Ja) => {
            "組み込み persona の場所がありません"
        }
        (TextKey::FactoryBuiltinPersonaMissing, Locale::En) => {
            "The built-in persona directory is missing."
        }
        (TextKey::WatchConfigFix, Locale::Ja) => "設定を修正してから、もう一度試してください",
        (TextKey::WatchConfigFix, Locale::En) => "Fix the settings, then try again.",
        (TextKey::WatchPermissionRequired, Locale::Ja) => "画面収録の許可が必要です",
        (TextKey::WatchPermissionRequired, Locale::En) => "Screen Recording permission is required.",
        (TextKey::WatchStartFailed, Locale::Ja) => "画面を見る処理を開始できませんでした",
        (TextKey::WatchStartFailed, Locale::En) => "Could not start screen monitoring.",
        (TextKey::WatchOperationFailed, Locale::Ja) => "画面を見る処理に失敗しました",
        (TextKey::WatchOperationFailed, Locale::En) => "Screen monitoring encountered an error.",
        (TextKey::WatchFullscreenConsent, Locale::Ja) => "画面全体を見てもいいですか？",
        (TextKey::WatchFullscreenConsent, Locale::En) => "May I monitor the entire screen?",
        (TextKey::WatchFullscreenDetail, Locale::Ja) => {
            "「いいえ」を選ぶと、見るアプリを設定できます。"
        }
        (TextKey::WatchFullscreenDetail, Locale::En) => {
            "Choose “No” to select which apps to monitor."
        }
        (TextKey::WatchTargetMissing, Locale::Ja) => "起動中のアプリが見つかりません",
        (TextKey::WatchTargetMissing, Locale::En) => "No running application was found.",
        (TextKey::WatchFullscreenTarget, Locale::Ja) => "フルスクリーン",
        (TextKey::WatchFullscreenTarget, Locale::En) => "Full screen",
        (TextKey::CaptureDispositionAccepted, Locale::Ja) => "撮影",
        (TextKey::CaptureDispositionAccepted, Locale::En) => "Captured",
        (TextKey::CaptureDispositionUnchanged, Locale::Ja) => "見送り（画面に変化なし）",
        (TextKey::CaptureDispositionUnchanged, Locale::En) => "Skipped (no screen change)",
        (TextKey::CaptureDispositionSuppressed, Locale::Ja) => "見送り（対象が無効です）",
        (TextKey::CaptureDispositionSuppressed, Locale::En) => "Skipped (target is disabled)",
        (TextKey::CaptureDispositionSelfApplication, Locale::Ja) => {
            "見送り（CooSenpAI が前面）"
        }
        (TextKey::CaptureDispositionSelfApplication, Locale::En) => {
            "Skipped (CooSenpAI is in the foreground)"
        }
        (TextKey::CaptureDispositionOwnBoundsUnavailable, Locale::Ja) => {
            "見送り（自ウィンドウの範囲を取得できません）"
        }
        (TextKey::CaptureDispositionOwnBoundsUnavailable, Locale::En) => {
            "Skipped (could not get the app window bounds)"
        }
        (TextKey::CaptureDispositionWindowUnavailable, Locale::Ja) => {
            "見送り（対象アプリのウィンドウがありません）"
        }
        (TextKey::CaptureDispositionWindowUnavailable, Locale::En) => {
            "Skipped (the target app has no window)"
        }
        (TextKey::CaptureDispositionMinSpacing, Locale::Ja) => {
            "見送り（撮影間隔が短すぎます）"
        }
        (TextKey::CaptureDispositionMinSpacing, Locale::En) => {
            "Skipped (capture interval is too short)"
        }
        (TextKey::FactRememberPrompt, Locale::Ja) => "これ、覚えておく？\n{text}",
        (TextKey::FactRememberPrompt, Locale::En) => "Should I remember this?\n{text}",
        (TextKey::FactCandidateMissing, Locale::Ja) => "確認する候補がありません",
        (TextKey::FactCandidateMissing, Locale::En) => "There is no candidate to confirm.",
        (TextKey::FactCandidateExpired, Locale::Ja) => "この候補の確認操作は期限切れです",
        (TextKey::FactCandidateExpired, Locale::En) => "This candidate is no longer available.",
        (TextKey::CaptureSendInProgress, Locale::Ja) => "範囲選択を送信中です",
        (TextKey::CaptureSendInProgress, Locale::En) => "The selection is being sent.",
        (TextKey::CaptureSelectedTextFailed, Locale::Ja) => "選択した文章を取得できませんでした",
        (TextKey::CaptureSelectedTextFailed, Locale::En) => "Could not get the selected text.",
        (TextKey::CaptureSelectedTextCopyFailed, Locale::Ja) => {
            "選択した文章をコピーできませんでした"
        }
        (TextKey::CaptureSelectedTextCopyFailed, Locale::En) => {
            "Could not copy the selected text."
        }
        (TextKey::CaptureClipboardReadFailed, Locale::Ja) => "クリップボードを読み取れませんでした",
        (TextKey::CaptureClipboardReadFailed, Locale::En) => "Could not read the clipboard.",
        (TextKey::CaptureImageReadFailed, Locale::Ja) => "選択した画像を読み込めませんでした",
        (TextKey::CaptureImageReadFailed, Locale::En) => "Could not read the selected image.",
        (TextKey::CaptureNoRegion, Locale::Ja) => "送信する範囲選択がありません",
        (TextKey::CaptureNoRegion, Locale::En) => "There is no selection to send.",
        (TextKey::CaptureImageLoadFailed, Locale::Ja) => "選択画像を読み込めません",
        (TextKey::CaptureImageLoadFailed, Locale::En) => "Could not load the selected image.",
        (TextKey::CaptureRegionMismatch, Locale::Ja) => "送信する範囲選択が一致しません",
        (TextKey::CaptureRegionMismatch, Locale::En) => {
            "The selection to send no longer matches."
        }
        (TextKey::CaptureTextUnavailable, Locale::Ja) => "送信できる文章がありません",
        (TextKey::CaptureTextUnavailable, Locale::En) => "There is no text to send.",
        (TextKey::CaptureAccessibilityRequired, Locale::Ja) => "アクセシビリティの許可が必要です",
        (TextKey::CaptureAccessibilityRequired, Locale::En) => "Accessibility permission is required.",
        (TextKey::CaptureShortcutDisabled, Locale::Ja) => "システム設定でスクリーンショットの『選択部分をクリップボードにコピー』を有効にしてください",
        (TextKey::CaptureShortcutDisabled, Locale::En) => "Enable 'Copy picture of selected area to the clipboard' in System Settings > Keyboard > Keyboard Shortcuts > Screenshots.",
        (TextKey::CaptureOperationFailed, Locale::Ja) => "範囲選択を処理できませんでした",
        (TextKey::CaptureOperationFailed, Locale::En) => "Could not process the selection.",
        (TextKey::CaptureAccessibilitySettingsFailed, Locale::Ja) => {
            "アクセシビリティのシステム設定を開けませんでした"
        }
        (TextKey::CaptureAccessibilitySettingsFailed, Locale::En) => {
            "Could not open Accessibility settings."
        }
        (TextKey::CaptureMessageTooLong, Locale::Ja) => "message が長すぎます",
        (TextKey::CaptureMessageTooLong, Locale::En) => "The message is too long.",
        (TextKey::ShortcutInvalid, Locale::Ja) => "ショートカット {shortcut} を解釈できません。",
        (TextKey::ShortcutInvalid, Locale::En) => "Could not parse shortcut {shortcut}.",
        (TextKey::ShortcutDuplicate, Locale::Ja) => {
            "ショートカット {shortcut} が別の操作と重複しています。"
        }
        (TextKey::ShortcutDuplicate, Locale::En) => {
            "Shortcut {shortcut} is already assigned to another action."
        }
        (TextKey::ShortcutRegisterFailed, Locale::Ja) => {
            "ショートカット {shortcut} は登録できません。別のキーを設定してください。"
        }
        (TextKey::ShortcutRegisterFailed, Locale::En) => {
            "Could not register shortcut {shortcut}. Choose another key."
        }
        (TextKey::ShortcutRestoreFailed, Locale::Ja) => {
            "{shortcut} の登録に失敗し、以前の {previous} も復元できませんでした"
        }
        (TextKey::ShortcutRestoreFailed, Locale::En) => {
            "Could not register {shortcut}; the previous shortcuts ({previous}) could not be restored."
        }
        (TextKey::SpeechSourceInvalid, Locale::Ja) => "source は composer で指定してください",
        (TextKey::SpeechSourceInvalid, Locale::En) => "source must be composer.",
        (TextKey::SpeechTextEmpty, Locale::Ja) => "text は空にできません",
        (TextKey::SpeechTextEmpty, Locale::En) => "Text cannot be empty.",
        (TextKey::SpeechTextTooLong, Locale::Ja) => "text が長すぎます",
        (TextKey::SpeechTextTooLong, Locale::En) => "Text is too long.",
        (TextKey::SpeechConfirmationMissing, Locale::Ja) => "確認する音声入力がありません",
        (TextKey::SpeechConfirmationMissing, Locale::En) => "There is no voice input to confirm.",
        (TextKey::SpeechStaleInput, Locale::Ja) => "古い音声入力です",
        (TextKey::SpeechStaleInput, Locale::En) => "This voice input is out of date.",
        (TextKey::SpeechRecognitionSettingsFailed, Locale::Ja) => {
            "音声認識のシステム設定を開けませんでした"
        }
        (TextKey::SpeechRecognitionSettingsFailed, Locale::En) => {
            "Could not open Speech Recognition settings."
        }
        (TextKey::SpeechMicrophoneSettingsFailed, Locale::Ja) => {
            "マイクのシステム設定を開けませんでした"
        }
        (TextKey::SpeechMicrophoneSettingsFailed, Locale::En) => {
            "Could not open Microphone settings."
        }
        (TextKey::SpeechSettingsKindInvalid, Locale::Ja) => {
            "kind は microphone または recognition です"
        }
        (TextKey::SpeechSettingsKindInvalid, Locale::En) => {
            "kind must be microphone or recognition."
        }
        (TextKey::SpeechSending, Locale::Ja) => "音声入力を送信中です",
        (TextKey::SpeechSending, Locale::En) => "Sending voice input.",
        (TextKey::SpeechEnding, Locale::Ja) => "音声入力を終了しています",
        (TextKey::SpeechEnding, Locale::En) => "Ending voice input.",
        (TextKey::SpeechGenerationMissing, Locale::Ja) => "音声入力の世代がありません",
        (TextKey::SpeechGenerationMissing, Locale::En) => "There is no active voice input generation.",
        (TextKey::SpeechStartFailed, Locale::Ja) => "音声入力を開始できません",
        (TextKey::SpeechStartFailed, Locale::En) => "Could not start voice input.",
        (TextKey::SpeechInputEmpty, Locale::Ja) => "音声入力が空です",
        (TextKey::SpeechInputEmpty, Locale::En) => "Voice input is empty.",
        (TextKey::SpeechHelperMissing, Locale::Ja) => "音声認識 helper が見つかりません",
        (TextKey::SpeechHelperMissing, Locale::En) => "The speech recognition helper was not found.",
        (TextKey::SpeechNoSpeech, Locale::Ja) => "音声を聞き取れませんでした",
        (TextKey::SpeechNoSpeech, Locale::En) => "No speech was detected.",
        (TextKey::SpeechGenericFailure, Locale::Ja) => "音声入力に失敗しました",
        (TextKey::SpeechGenericFailure, Locale::En) => "Voice input failed.",
        (TextKey::SpeechMicrophoneDenied, Locale::Ja) => "マイクの使用が許可されていません",
        (TextKey::SpeechMicrophoneDenied, Locale::En) => "Microphone access is not permitted.",
        (TextKey::SpeechMicrophoneRestricted, Locale::Ja) => "マイクの使用が制限されています",
        (TextKey::SpeechMicrophoneRestricted, Locale::En) => "Microphone access is restricted.",
        (TextKey::SpeechMicrophoneUnavailable, Locale::Ja) => "マイクを利用できません",
        (TextKey::SpeechMicrophoneUnavailable, Locale::En) => "The microphone is unavailable.",
        (TextKey::SpeechRecognitionDenied, Locale::Ja) => "音声認識の使用が許可されていません",
        (TextKey::SpeechRecognitionDenied, Locale::En) => "Speech Recognition access is not permitted.",
        (TextKey::SpeechRecognitionRestricted, Locale::Ja) => "音声認識の使用が制限されています",
        (TextKey::SpeechRecognitionRestricted, Locale::En) => "Speech Recognition access is restricted.",
        (TextKey::SpeechRecognitionUnavailable, Locale::Ja) => "音声認識を利用できません",
        (TextKey::SpeechRecognitionUnavailable, Locale::En) => "Speech Recognition is unavailable.",
        (TextKey::SpeechLocaleUnavailable, Locale::Ja) => "指定したロケールの音声認識は利用できません",
        (TextKey::SpeechLocaleUnavailable, Locale::En) => "Speech Recognition is unavailable for this locale.",
        (TextKey::SpeechOnDeviceUnsupported, Locale::Ja) => {
            "このロケールではオンデバイス音声認識を利用できません"
        }
        (TextKey::SpeechOnDeviceUnsupported, Locale::En) => {
            "On-device Speech Recognition is unavailable for this locale."
        }
        (TextKey::SpeechInputDeviceUnavailable, Locale::Ja) => "音声入力デバイスを利用できません",
        (TextKey::SpeechInputDeviceUnavailable, Locale::En) => "The voice input device is unavailable.",
        (TextKey::SpeechInputDeviceListFailed, Locale::Ja) => "マイク一覧を取得できません",
        (TextKey::SpeechInputDeviceListFailed, Locale::En) => "Could not get the microphone list.",
        (TextKey::SpeechInputDeviceListFallback, Locale::Ja) => {
            "マイク一覧を取得できないため、システム既定を使います"
        }
        (TextKey::SpeechInputDeviceListFallback, Locale::En) => {
            "The microphone list could not be retrieved, so the system default will be used."
        }
        (TextKey::SpeechKeyStateFailed, Locale::Ja) => {
            "マイクキーの状態を確認できないため録音を終了します"
        }
        (TextKey::SpeechKeyStateFailed, Locale::En) => {
            "Recording will end because the microphone key state is unavailable."
        }
        (TextKey::SpeechInputDeviceFallback, Locale::Ja) => {
            "選択したマイクが見つからないため、システム既定を使います"
        }
        (TextKey::SpeechInputDeviceFallback, Locale::En) => {
            "The selected microphone was not found, so the system default will be used."
        }
        (TextKey::SpeechInputDeviceFallbackShort, Locale::Ja) => "システム既定を使います",
        (TextKey::SpeechInputDeviceFallbackShort, Locale::En) => {
            "The system default will be used."
        }
        (TextKey::ScreenPermissionAllow, Locale::Ja) => {
            "システム設定の画面収録で CooSenpAI を許可して、アプリを再起動してください"
        }
        (TextKey::ScreenPermissionAllow, Locale::En) => {
            "Allow CooSenpAI under Screen Recording in System Settings, then restart the app."
        }
        (TextKey::ScreenPermissionRestart, Locale::Ja) => {
            "画面収録は許可済みですが、反映にはアプリの再起動が必要です"
        }
        (TextKey::ScreenPermissionRestart, Locale::En) => {
            "Screen Recording is allowed, but the app must be restarted for it to take effect."
        }
        (TextKey::ScreenPermissionRestricted, Locale::Ja) => {
            "この Mac の制限により画面収録を利用できません"
        }
        (TextKey::ScreenPermissionRestricted, Locale::En) => {
            "Screen Recording is unavailable because of restrictions on this Mac."
        }
        (TextKey::ScreenPermissionUnavailable, Locale::Ja) => {
            "画面収録の権限状態を確認できません"
        }
        (TextKey::ScreenPermissionUnavailable, Locale::En) => {
            "Could not determine the Screen Recording permission status."
        }
        (TextKey::AudioSourceRequired, Locale::Ja) => "マイクまたはスピーカーを選択してください",
        (TextKey::AudioSourceRequired, Locale::En) => "Choose a microphone or speaker.",
        (TextKey::AudioScreenPermissionRequired, Locale::Ja) => {
            "スピーカーの音を聞くには画面収録の許可が必要です"
        }
        (TextKey::AudioScreenPermissionRequired, Locale::En) => {
            "Screen Recording permission is required to hear speaker audio."
        }
        (TextKey::AudioSystemPermissionRequired, Locale::Ja) => "スピーカーの音を聞くには、システム設定の「画面収録とシステムオーディオ録音」でシステムオーディオ録音を許可してください。",
        (TextKey::AudioSystemPermissionRequired, Locale::En) => "Allow System Audio Recording in System Settings > Screen & System Audio Recording to hear speaker audio.",
        (TextKey::AudioSystemFailed, Locale::Ja) => "スピーカー音声を取得できません。システムオーディオ録音の許可と出力デバイスを確認してください。",
        (TextKey::AudioSystemFailed, Locale::En) => "Could not capture speaker audio. Check System Audio Recording permission and the output device.",
        (TextKey::AudioSystemDeviceUnavailable, Locale::Ja) => "音声出力デバイスがありません。出力デバイスの接続を確認してください。",
        (TextKey::AudioSystemDeviceUnavailable, Locale::En) => "No audio output device is available. Check the output device connection.",
        (TextKey::AudioSystemFormatFailed, Locale::Ja) => "スピーカー音声の形式を取得できません。出力デバイスを確認し、Hearing AI を入れ直してください。",
        (TextKey::AudioSystemFormatFailed, Locale::En) => "Could not read the speaker audio format. Check the output device and restart Hearing AI.",
        (TextKey::AudioSystemOverflow, Locale::Ja) => "スピーカー音声の処理が追いつかず停止しました。負荷を減らして Hearing AI を入れ直してください。",
        (TextKey::AudioSystemOverflow, Locale::En) => "Speaker capture stopped because processing could not keep up. Reduce system load and restart Hearing AI.",
        (TextKey::AudioSystemStartupTimeout, Locale::Ja) => "スピーカー音声の開始がタイムアウトしました。システムオーディオ録音の許可と出力デバイスを確認してください。",
        (TextKey::AudioSystemStartupTimeout, Locale::En) => "Speaker capture timed out during startup. Check System Audio Recording permission and the output device.",
        (TextKey::AudioHelperMissing, Locale::Ja) => "coosenpai-hearing が見つかりません",
        (TextKey::AudioHelperMissing, Locale::En) => "The coosenpai-hearing helper was not found.",
        (TextKey::AudioHelperUnexpectedExit, Locale::Ja) => "聴覚観察 helper が予期せず終了しました",
        (TextKey::AudioHelperUnexpectedExit, Locale::En) => "The hearing helper exited unexpectedly.",
        (TextKey::AudioObservationSaveFailed, Locale::Ja) => {
            "確定した音声を観察として保存できませんでした"
        }
        (TextKey::AudioObservationSaveFailed, Locale::En) => {
            "Could not save the confirmed audio as an observation."
        }
        (TextKey::AudioWorkerStopped, Locale::Ja) => "確定した音声観察の配達 worker が停止しています",
        (TextKey::AudioWorkerStopped, Locale::En) => "The confirmed audio delivery worker has stopped.",
        (TextKey::AudioWarningGeneric, Locale::Ja) => "音声処理の警告",
        (TextKey::AudioWarningGeneric, Locale::En) => "Audio processing warning.",
        (TextKey::SetupSelectionEmpty, Locale::Ja) => "選択値は空にできません",
        (TextKey::SetupSelectionEmpty, Locale::En) => "The selection cannot be empty.",
        (TextKey::SetupSavedResponseUnavailable, Locale::Ja) => {
            "保存済みの返事を表示済みにできませんでした"
        }
        (TextKey::SetupSavedResponseUnavailable, Locale::En) => {
            "Could not mark the saved response as displayed."
        }
        (TextKey::SetupResponsePending, Locale::Ja) => {
            "返事の表示が終わるまで、この案内を進められません"
        }
        (TextKey::SetupResponsePending, Locale::En) => {
            "This guide cannot continue until the response finishes displaying."
        }
        (TextKey::SetupProviderInvalid, Locale::Ja) => "provider が不正です",
        (TextKey::SetupProviderInvalid, Locale::En) => "The provider is invalid.",
        (TextKey::SetupApiKeyEmpty, Locale::Ja) => "API キーは空欄にできません",
        (TextKey::SetupApiKeyEmpty, Locale::En) => "The API key cannot be empty.",
        (TextKey::SetupLoginApiKeyRejected, Locale::Ja) => {
            "ログイン接続に API キーは指定できません"
        }
        (TextKey::SetupLoginApiKeyRejected, Locale::En) => {
            "An API key cannot be provided for a login connection."
        }
        (TextKey::SetupConfigUnavailable, Locale::Ja) => "初回セットアップを開始できません",
        (TextKey::SetupConfigUnavailable, Locale::En) => "Initial setup cannot be started.",
        (TextKey::SettingsNumericRequired, Locale::Ja) => "数値で指定してください。",
        (TextKey::SettingsNumericRequired, Locale::En) => "Enter a number.",
        (TextKey::SettingsUnsignedBuildOnly, Locale::Ja) => {
            "OS 通知は署名済みビルドでのみ選択できます。"
        }
        (TextKey::SettingsUnsignedBuildOnly, Locale::En) => {
            "OS notifications are available only in signed builds."
        }
        (TextKey::AttachmentReadFailed, Locale::Ja) => "添付画像を読み込めません",
        (TextKey::AttachmentReadFailed, Locale::En) => "Could not load the attached image.",
        (TextKey::ProcessNoFrontmostApplication, Locale::Ja) => "起動中のアプリが見つかりません",
        (TextKey::ProcessNoFrontmostApplication, Locale::En) => "No frontmost application was found.",
        (TextKey::ConfigInvalidObject, Locale::Ja) => "設定はオブジェクトで指定してください。",
        (TextKey::ConfigInvalidObject, Locale::En) => "Enter an object for this setting.",
        (TextKey::ConfigInvalidArray, Locale::Ja) => "配列で指定してください。",
        (TextKey::ConfigInvalidArray, Locale::En) => "Enter an array for this setting.",
        (TextKey::ConfigUnknownKey, Locale::Ja) => "未知のキーです。",
        (TextKey::ConfigUnknownKey, Locale::En) => "This key is not recognized.",
        (TextKey::ConfigRequired, Locale::Ja) => "必須です。",
        (TextKey::ConfigRequired, Locale::En) => "This value is required.",
        (TextKey::ConfigPositiveInteger, Locale::Ja) => "正の整数で指定してください。",
        (TextKey::ConfigPositiveInteger, Locale::En) => "Enter a positive integer.",
        (TextKey::ConfigNonnegativeInteger, Locale::Ja) => "0以上の整数で指定してください。",
        (TextKey::ConfigNonnegativeInteger, Locale::En) => "Enter a non-negative integer.",
        (TextKey::ConfigOptionalNonnegativeInteger, Locale::Ja) => {
            "0以上の整数または null で指定してください。"
        }
        (TextKey::ConfigOptionalNonnegativeInteger, Locale::En) => {
            "Enter a non-negative integer or null."
        }
        (TextKey::ConfigPositiveNumber, Locale::Ja) => "正の数で指定してください。",
        (TextKey::ConfigPositiveNumber, Locale::En) => "Enter a positive number.",
        (TextKey::ConfigNonemptyString, Locale::Ja) => "空でない文字列で指定してください。",
        (TextKey::ConfigNonemptyString, Locale::En) => "Enter a non-empty string.",
        (TextKey::ConfigNonemptyOrNullString, Locale::Ja) => {
            "空でない文字列または null で指定してください。"
        }
        (TextKey::ConfigNonemptyOrNullString, Locale::En) => {
            "Enter a non-empty string or null."
        }
        (TextKey::ConfigColor, Locale::Ja) => "#RRGGBB 形式で指定してください。",
        (TextKey::ConfigColor, Locale::En) => "Enter a color in #RRGGBB format.",
        (TextKey::ConfigAvatarPath, Locale::Ja) => {
            "state 配下の png / jpg / jpeg ファイルパスで指定してください。"
        }
        (TextKey::ConfigAvatarPath, Locale::En) => {
            "Enter a png, jpg, or jpeg file path under state."
        }
        (TextKey::ConfigWhitespaceString, Locale::Ja) => {
            "空白以外の文字列で指定してください。"
        }
        (TextKey::ConfigWhitespaceString, Locale::En) => "Enter a non-blank string.",
        (TextKey::ConfigAllowedValues, Locale::Ja) => "{values} のいずれかで指定してください。",
        (TextKey::ConfigAllowedValues, Locale::En) => "Choose one of: {values}.",
        (TextKey::ConfigEither, Locale::Ja) => {
            "{first} または {second} で指定してください。"
        }
        (TextKey::ConfigEither, Locale::En) => "Choose either {first} or {second}.",
        (TextKey::ConfigPersonaId, Locale::Ja) => "性格のIDが不正です。",
        (TextKey::ConfigPersonaId, Locale::En) => "The persona ID is invalid.",
        (TextKey::ConfigExecutable, Locale::Ja) => {
            "実行ファイルは絶対パスまたは null で指定してください。"
        }
        (TextKey::ConfigExecutable, Locale::En) => {
            "Enter an absolute executable path or null."
        }
        (TextKey::ConfigRange, Locale::Ja) => "{minimum}以上{maximum}以下の整数で指定してください。",
        (TextKey::ConfigRange, Locale::En) => {
            "Enter an integer from {minimum} through {maximum}."
        }
        (TextKey::ConfigPositiveRange, Locale::Ja) => "{minimum}以上の整数で指定してください。",
        (TextKey::ConfigPositiveRange, Locale::En) => {
            "Enter an integer of {minimum} or greater."
        }
        (TextKey::ConfigActiveThreshold, Locale::Ja) => {
            "typingPauseMs より小さくしてください。"
        }
        (TextKey::ConfigActiveThreshold, Locale::En) => {
            "Enter a value smaller than typingPauseMs."
        }
        (TextKey::ConfigSpacing, Locale::Ja) => {
            "pollMs 以上かつ maxIntervalMs 以下で指定してください。"
        }
        (TextKey::ConfigSpacing, Locale::En) => {
            "Enter a value at least pollMs and at most maxIntervalMs."
        }
        (TextKey::ConfigDuplicate, Locale::Ja) => "同じ値を複数回指定できません。",
        (TextKey::ConfigDuplicate, Locale::En) => "The same value cannot be specified more than once.",
        (TextKey::ConfigBundleId, Locale::Ja) => {
            "空でない制御文字を含まない bundle ID を指定してください。"
        }
        (TextKey::ConfigBundleId, Locale::En) => {
            "Enter a non-empty bundle ID without control characters."
        }
        (TextKey::ConfigAppName, Locale::Ja) => {
            "空でない制御文字を含まないアプリ名を指定してください。"
        }
        (TextKey::ConfigAppName, Locale::En) => {
            "Enter a non-empty app name without control characters."
        }
        (TextKey::ConfigShortcut, Locale::Ja) => {
            "1以上64以下のショートカット文字列または null で指定してください。"
        }
        (TextKey::ConfigShortcut, Locale::En) => {
            "Enter a shortcut string of 1–64 characters or null."
        }
        (TextKey::ConfigTauriShortcut, Locale::Ja) => {
            "Tauri が解釈できるショートカットを指定してください。"
        }
        (TextKey::ConfigTauriShortcut, Locale::En) => {
            "Enter a shortcut that Tauri can interpret."
        }
        (TextKey::ConfigEscapeShortcut, Locale::Ja) => {
            "Escape は録音の取り消しに使うため設定できません。"
        }
        (TextKey::ConfigEscapeShortcut, Locale::En) => {
            "Escape cannot be assigned because it cancels recording."
        }
        (TextKey::ConfigName, Locale::Ja) => {
            "1以上20以下の制御文字を含まない名前で指定してください。"
        }
        (TextKey::ConfigName, Locale::En) => {
            "Enter a name of 1–20 characters without control characters."
        }
        (TextKey::ConfigReviewTime, Locale::Ja) => "空文字または HH:MM 形式で指定してください。",
        (TextKey::ConfigReviewTime, Locale::En) => "Enter an empty value or a time in HH:MM format.",
        (TextKey::ConfigReminderLimit, Locale::Ja) => "10件以下で指定してください。",
        (TextKey::ConfigReminderLimit, Locale::En) => "Enter no more than 10 reminders.",
        (TextKey::ConfigReminderId, Locale::Ja) => {
            "1以上128以下の英数字とハイフンで指定してください。"
        }
        (TextKey::ConfigReminderId, Locale::En) => {
            "Enter 1–128 letters, numbers, or hyphens."
        }
        (TextKey::ConfigReminderTime, Locale::Ja) => "HH:MM 形式で指定してください。",
        (TextKey::ConfigReminderTime, Locale::En) => "Enter a time in HH:MM format.",
        (TextKey::ConfigReminderTheme, Locale::Ja) => {
            "1以上500以下の制御文字を含まない文字列で指定してください。"
        }
        (TextKey::ConfigReminderTheme, Locale::En) => {
            "Enter a 1–500 character string without control characters."
        }
        (TextKey::ConfigLanguage, Locale::Ja) => "ja または en で指定してください。",
        (TextKey::ConfigLanguage, Locale::En) => "Choose either ja or en.",
        (TextKey::ConfigVoiceOutputProvider, Locale::Ja) => "system または voicevox で指定してください。",
        (TextKey::ConfigVoiceOutputProvider, Locale::En) => {
            "The voice-output provider must be system or voicevox."
        }
        (TextKey::ConfigInvalidValue, Locale::Ja) => "設定値が不正です。",
        (TextKey::ConfigInvalidValue, Locale::En) => "This setting value is invalid.",
        (TextKey::ConfigFileRead, Locale::Ja) => "設定ファイルを読み込めません",
        (TextKey::ConfigFileRead, Locale::En) => "Could not read the configuration file.",
        (TextKey::ConfigJson, Locale::Ja) => "設定 JSON が不正です",
        (TextKey::ConfigJson, Locale::En) => "The configuration JSON is invalid.",
        (TextKey::ConfigValidation, Locale::Ja) => "設定の検証に失敗しました",
        (TextKey::ConfigValidation, Locale::En) => "Configuration validation failed.",
        (TextKey::ConfigVersionInteger, Locale::Ja) => "3 の整数で指定してください。",
        (TextKey::ConfigVersionInteger, Locale::En) => "Enter the integer 3.",
        (TextKey::ConfigUnsupportedVersion, Locale::Ja) => "設定バージョン {version} は未対応です",
        (TextKey::ConfigUnsupportedVersion, Locale::En) => {
            "Configuration version {version} is not supported."
        }
        (TextKey::ConfigRevisionConflict, Locale::Ja) => {
            "設定が別の場所で変更されました。読み直してください"
        }
        (TextKey::ConfigRevisionConflict, Locale::En) => {
            "The configuration changed elsewhere. Reload it and try again."
        }
        (TextKey::ConfigLock, Locale::Ja) => "設定の lock を取得できません",
        (TextKey::ConfigLock, Locale::En) => "Could not acquire the configuration lock.",
        (TextKey::ConfigCandidateBuildFailed, Locale::Ja) => {
            "設定候補を構築できませんでした"
        }
        (TextKey::ConfigCandidateBuildFailed, Locale::En) => {
            "Could not build the configuration candidate."
        }
        (TextKey::SetupBubbleDisplayFailed, Locale::Ja) => {
            "初回セットアップを表示できません: {error}"
        }
        (TextKey::SetupBubbleDisplayFailed, Locale::En) => {
            "Could not display initial setup: {error}"
        }
        (TextKey::SetupLanguageIntro, Locale::Ja) => "ようこそ。まず表示言語を選んでください。",
        (TextKey::SetupLanguageIntro, Locale::En) => {
            "Welcome. First, choose a display language."
        }
        (TextKey::SetupLanguageJa, _) => "日本語",
        (TextKey::SetupLanguageEn, _) => "English",
        (TextKey::SetupLanguageNext, Locale::Ja) => "次へ",
        (TextKey::SetupLanguageNext, Locale::En) => "Next",
        (TextKey::SetupLanguageRequired, Locale::Ja) => "言語を選んでください",
        (TextKey::SetupLanguageRequired, Locale::En) => "Choose a language.",
        (TextKey::SetupLanguageInvalid, Locale::Ja) => "language が不正です",
        (TextKey::SetupLanguageInvalid, Locale::En) => "The language is invalid.",
        (TextKey::SetupLanguageSelectionUnavailable, Locale::Ja) => {
            "初回セットアップの言語選択を受け付けられません"
        }
        (TextKey::SetupLanguageSelectionUnavailable, Locale::En) => {
            "The initial setup is not accepting a language selection."
        }
        (TextKey::SetupExpired, Locale::Ja) => "この吹き出しの操作は期限切れです",
        (TextKey::SetupExpired, Locale::En) => "This bubble is no longer accepting input.",
        (TextKey::TutorialCurrentStepMissing, Locale::Ja) => {
            "現在のチュートリアル step がありません"
        }
        (TextKey::TutorialCurrentStepMissing, Locale::En) => {
            "There is no current tutorial step."
        }
        (TextKey::TutorialAutoAdvance, Locale::Ja) => "この案内は自動で進みます",
        (TextKey::TutorialAutoAdvance, Locale::En) => "This guide advances automatically.",
        (TextKey::SetupProviderRequired, Locale::Ja) => "provider を選んでください",
        (TextKey::SetupProviderRequired, Locale::En) => "Choose a provider.",
        (TextKey::SetupProviderSelectionUnavailable, Locale::Ja) => {
            "初回セットアップの provider 選択を受け付けられません"
        }
        (TextKey::SetupProviderSelectionUnavailable, Locale::En) => {
            "The initial setup is not accepting a provider selection."
        }
        (TextKey::SetupConnectionMethodRequired, Locale::Ja) => "接続方法を選んでください",
        (TextKey::SetupConnectionMethodRequired, Locale::En) => "Choose a connection method.",
        (TextKey::SetupConnectionMethodSelectionUnavailable, Locale::Ja) => {
            "初回セットアップの接続方法選択を受け付けられません"
        }
        (TextKey::SetupConnectionMethodSelectionUnavailable, Locale::En) => {
            "The initial setup is not accepting a connection method selection."
        }
        (TextKey::SetupApiKeyRequired, Locale::Ja) => "API キーを入力してください",
        (TextKey::SetupApiKeyRequired, Locale::En) => "Enter an API key.",
        (TextKey::SetupApiKeyInvalid, Locale::Ja) => "API キーに無効な文字が含まれています",
        (TextKey::SetupApiKeyInvalid, Locale::En) => "The API key contains invalid characters.",
        (TextKey::SetupApiKeyNotAccepted, Locale::Ja) => "API キー接続を受け付けられません",
        (TextKey::SetupApiKeyNotAccepted, Locale::En) => "An API key connection is not accepted here.",
        (TextKey::SetupResponseUnavailable, Locale::Ja) => "初回セットアップの応答を受け付けられません",
        (TextKey::SetupResponseUnavailable, Locale::En) => {
            "The initial setup is not accepting this response."
        }
        (TextKey::InvalidBubbleAction, Locale::Ja) => "吹き出しの操作が不正です",
        (TextKey::InvalidBubbleAction, Locale::En) => "The bubble action is invalid.",
        (TextKey::SetupConnectionTimeout, Locale::Ja) => "接続確認がタイムアウトしました",
        (TextKey::SetupConnectionTimeout, Locale::En) => "The connection check timed out.",
        (TextKey::SetupUnavailable, Locale::Ja) => "初回セットアップを開始できません",
        (TextKey::SetupUnavailable, Locale::En) => "Setup cannot be started.",
        (TextKey::SetupNext, Locale::Ja) => "次へ",
        (TextKey::SetupNext, Locale::En) => "Next",
        (TextKey::SetupApiKeyLabel, Locale::Ja) => "API キー",
        (TextKey::SetupApiKeyLabel, Locale::En) => "API key",
        (TextKey::SetupApiKeyPlaceholder, Locale::Ja) => "API キーを入力",
        (TextKey::SetupApiKeyPlaceholder, Locale::En) => "Enter an API key",
        (TextKey::SetupConnectionConfirm, Locale::Ja) => "接続方法を決定",
        (TextKey::SetupConnectionConfirm, Locale::En) => "Confirm connection method",
        (TextKey::SetupApiKeySubmit, Locale::Ja) => "接続を確認",
        (TextKey::SetupApiKeySubmit, Locale::En) => "Check connection",
        (TextKey::SetupLoginCodex, Locale::Ja) => "Codex のログイン",
        (TextKey::SetupLoginCodex, Locale::En) => "Log in to Codex",
        (TextKey::SetupLoginClaude, Locale::Ja) => "Claude Code のログイン",
        (TextKey::SetupLoginClaude, Locale::En) => "Log in to Claude Code",
        (TextKey::SetupLoginCli, Locale::Ja) => "CLI のログイン",
        (TextKey::SetupLoginCli, Locale::En) => "Log in to the CLI",
        (TextKey::SetupSettings, Locale::Ja) => "設定を開く",
        (TextKey::SetupSettings, Locale::En) => "Open settings",
        (TextKey::SetupRetry, Locale::Ja) => "もう一度調べる",
        (TextKey::SetupRetry, Locale::En) => "Check again",
        (TextKey::SetupNoneDetail, Locale::Ja) => {
            "Codex CLI または Claude Code をインストールしてログインするか、設定画面で API キーを保存したあと、もう一度調べてください。"
        }
        (TextKey::SetupNoneDetail, Locale::En) => {
            "Install and log in to Codex CLI or Claude Code, or save an API key in Settings, then check again."
        }
        (TextKey::TutorialCancelled, Locale::Ja) => "チュートリアルを取り消しました",
        (TextKey::TutorialCancelled, Locale::En) => "The tutorial was cancelled.",
        (TextKey::TutorialObserverActivity, Locale::Ja) => "チュートリアル中",
        (TextKey::TutorialObserverActivity, Locale::En) => "During the tutorial",
        (TextKey::TutorialManualDismissNotAllowed, Locale::Ja) => {
            "チュートリアルの案内は手動で閉じられません"
        }
        (TextKey::TutorialManualDismissNotAllowed, Locale::En) => {
            "The tutorial guide cannot be dismissed manually."
        }
        (TextKey::TutorialSettingsPresentationUnavailable, Locale::Ja) => {
            "設定画面の表示確認を受け付けられません"
        }
        (TextKey::TutorialSettingsPresentationUnavailable, Locale::En) => {
            "The settings presentation confirmation is not accepted."
        }
        (TextKey::CommandShuttingDown, Locale::Ja) => "終了処理中です",
        (TextKey::CommandShuttingDown, Locale::En) => "The app is shutting down.",
        (TextKey::CommandTransitionInProgress, Locale::Ja) => "設定の反映処理中です",
        (TextKey::CommandTransitionInProgress, Locale::En) => "A settings transition is in progress.",
        (TextKey::CommandTutorialFinishing, Locale::Ja) => "終了処理をやり直してください",
        (TextKey::CommandTutorialFinishing, Locale::En) => "Please retry the finishing step.",
        (TextKey::CommandSetupRequired, Locale::Ja) => "初期設定を完了してください",
        (TextKey::CommandSetupRequired, Locale::En) => "Complete the initial setup.",
        (TextKey::CommandTutorialOperationNotAllowed, Locale::Ja) => {
            "この操作は現在の案内では使えません"
        }
        (TextKey::CommandTutorialOperationNotAllowed, Locale::En) => {
            "This action is not available in the current guide."
        }
        (TextKey::CommandStaleGeneration, Locale::Ja) => "古い操作の完了は反映されませんでした",
        (TextKey::CommandStaleGeneration, Locale::En) => {
            "The completion of an outdated operation was ignored."
        }
        (TextKey::CommandRuntimeUnavailable, Locale::Ja) => "設定エラーで停止中です",
        (TextKey::CommandRuntimeUnavailable, Locale::En) => "The app is stopped because of a settings error.",
        (TextKey::CommandInvalidInput, Locale::Ja) => "この状態では操作できません",
        (TextKey::CommandInvalidInput, Locale::En) => "This action is not available in the current state.",
        (TextKey::CommandWindowNotAllowed, Locale::Ja) => "このウィンドウからは操作できません",
        (TextKey::CommandWindowNotAllowed, Locale::En) => {
            "This action is not available from this window."
        }
        (TextKey::CommandResultUnavailable, Locale::Ja) => "処理結果を確認できませんでした",
        (TextKey::CommandResultUnavailable, Locale::En) => "The operation result could not be confirmed.",
        (TextKey::IdentifierEmpty, Locale::Ja) => "id は空にできません",
        (TextKey::IdentifierEmpty, Locale::En) => "The id cannot be empty.",
        (TextKey::MessageEmpty, Locale::Ja) => "message は空にできません",
        (TextKey::MessageEmpty, Locale::En) => "The message cannot be empty.",
        (TextKey::MessageTooLong, Locale::Ja) => "message が長すぎます",
        (TextKey::MessageTooLong, Locale::En) => "The message is too long.",
        (TextKey::ModelPopupOpenFailed, Locale::Ja) => "モデル変更を開けません: {error}",
        (TextKey::ModelPopupOpenFailed, Locale::En) => "Could not open model settings: {error}",
        (TextKey::ModelPopupCloseFailed, Locale::Ja) => "モデル変更を閉じられません: {error}",
        (TextKey::ModelPopupCloseFailed, Locale::En) => "Could not close model settings: {error}",
        (TextKey::DetailsOpenFailed, Locale::Ja) => "詳細を開けません: {error}",
        (TextKey::DetailsOpenFailed, Locale::En) => "Could not open details: {error}",
        (TextKey::DetailsDataflowLogReadFailed, Locale::Ja) => {
            "観察ログを読み込めません: {error}"
        }
        (TextKey::DetailsDataflowLogReadFailed, Locale::En) => {
            "Could not read the observation log: {error}"
        }
        (TextKey::DetailsDataflowPathOpenFailed, Locale::Ja) => "参照元を開けませんでした",
        (TextKey::DetailsDataflowPathOpenFailed, Locale::En) => "Could not open the referenced file.",
        (TextKey::ModelConfigObject, Locale::Ja) => "モデル設定はオブジェクトで指定してください",
        (TextKey::ModelConfigObject, Locale::En) => "Model settings must be an object.",
        (TextKey::ModelConfigCompanionOnly, Locale::Ja) => {
            "モデル設定では companion だけを変更できます"
        }
        (TextKey::ModelConfigCompanionOnly, Locale::En) => {
            "Only companion can be changed in model settings."
        }
        (TextKey::ModelConfigCompanionObject, Locale::Ja) => {
            "モデル設定の companion はオブジェクトで指定してください"
        }
        (TextKey::ModelConfigCompanionObject, Locale::En) => {
            "The companion model settings must be an object."
        }
        (TextKey::ModelConfigRequired, Locale::Ja) => "モデル設定を一つ以上指定してください",
        (TextKey::ModelConfigRequired, Locale::En) => "Specify at least one model setting.",
        (TextKey::ModelConfigKeys, Locale::Ja) => {
            "モデル設定では provider、model、effort だけを変更できます"
        }
        (TextKey::ModelConfigKeys, Locale::En) => {
            "Only provider, model, and effort can be changed in model settings."
        }
        (TextKey::AvatarImageProcessFailed, Locale::Ja) => "アバター画像を処理できません: {error}",
        (TextKey::AvatarImageProcessFailed, Locale::En) => "Could not process the avatar image: {error}",
        (TextKey::AvatarImageProcessingIncomplete, Locale::Ja) => {
            "アバター画像の処理を完了できません: {error}"
        }
        (TextKey::AvatarImageProcessingIncomplete, Locale::En) => {
            "Could not finish processing the avatar image: {error}"
        }
        (TextKey::AvatarImageStageFailed, Locale::Ja) => "アバター画像を一時保存できません: {error}",
        (TextKey::AvatarImageStageFailed, Locale::En) => "Could not stage the avatar image: {error}",
        (TextKey::AvatarImageStageIncomplete, Locale::Ja) => {
            "アバター画像の一時保存を完了できません: {error}"
        }
        (TextKey::AvatarImageStageIncomplete, Locale::En) => {
            "Could not finish staging the avatar image: {error}"
        }
        (TextKey::AvatarWindowMissing, Locale::Ja) => "アバターウィンドウが見つかりません",
        (TextKey::AvatarWindowMissing, Locale::En) => "The avatar window was not found.",
        (TextKey::AvatarPositionFailed, Locale::Ja) => "アバターの位置を設定できません: {error}",
        (TextKey::AvatarPositionFailed, Locale::En) => "Could not position the avatar: {error}",
        (TextKey::AvatarVisibilityFailed, Locale::Ja) => {
            "アバターの表示を変更できません: {error}"
        }
        (TextKey::AvatarVisibilityFailed, Locale::En) => {
            "Could not change avatar visibility: {error}"
        }
        (TextKey::AssertivenessInvalid, Locale::Ja) => "積極性の値が不正です",
        (TextKey::AssertivenessInvalid, Locale::En) => "The assertiveness value is invalid.",
        (TextKey::PersonaNotFound, Locale::Ja) => "選択した性格が見つかりません",
        (TextKey::PersonaNotFound, Locale::En) => "The selected persona was not found.",
        (TextKey::PersonaChangedNotice, Locale::Ja) => {
            "ペルソナが {previous} から {next} に切り替わった"
        }
        (TextKey::PersonaChangedNotice, Locale::En) => {
            "The persona changed from {previous} to {next}."
        }
        (TextKey::SystemSettingsOpenFailed, Locale::Ja) => "システム設定を開けませんでした",
        (TextKey::SystemSettingsOpenFailed, Locale::En) => "Could not open System Settings.",
        (TextKey::LicenseDocumentOpenFailed, Locale::Ja) => "ライセンス文書を開けませんでした",
        (TextKey::LicenseDocumentOpenFailed, Locale::En) => "Could not open the license document.",
        (TextKey::TutorialPersonaWaiting, Locale::Ja) => "性格の案内までお待ちください",
        (TextKey::TutorialPersonaWaiting, Locale::En) => "Wait until the persona guide is ready.",
        (TextKey::BubbleAppearancePreviewInvalid, Locale::Ja) => "見た目のプレビュー値が不正です",
        (TextKey::BubbleAppearancePreviewInvalid, Locale::En) => {
            "The appearance preview value is invalid."
        }
        (TextKey::BubbleRendererAttemptInvalid, Locale::Ja) => "renderer 初期化回数が不正です",
        (TextKey::BubbleRendererAttemptInvalid, Locale::En) => {
            "The renderer initialization count is invalid."
        }
        (TextKey::BubbleGenerationUnknown, Locale::Ja) => "未知の吹き出しgenerationです",
        (TextKey::BubbleGenerationUnknown, Locale::En) => "The bubble generation is unknown.",
        (TextKey::BubbleHeightOutOfRange, Locale::Ja) => "吹き出しの高さが範囲外です",
        (TextKey::BubbleHeightOutOfRange, Locale::En) => "The bubble height is out of range.",
        (TextKey::BubbleResizeFailed, Locale::Ja) => "吹き出しの大きさを変更できません",
        (TextKey::BubbleResizeFailed, Locale::En) => "Could not resize the bubble.",
        (TextKey::MemoryPeriodInvalid, Locale::Ja) => {
            "period は YYYY-MM-DD または YYYY-Www で指定してください"
        }
        (TextKey::MemoryPeriodInvalid, Locale::En) => {
            "period must be YYYY-MM-DD or YYYY-Www."
        }
        (TextKey::MemoryOperationFailed, Locale::Ja) => "記憶の処理に失敗しました: {error}",
        (TextKey::MemoryOperationFailed, Locale::En) => "The memory operation failed.",
        (TextKey::PersonaOperationFailed, Locale::Ja) => "性格の処理に失敗しました: {error}",
        (TextKey::PersonaOperationFailed, Locale::En) => "The persona operation failed.",
        (TextKey::RunningApplicationsListFailed, Locale::Ja) => {
            "起動中のアプリ一覧を取得できません: {error}"
        }
        (TextKey::RunningApplicationsListFailed, Locale::En) => {
            "Could not get the list of running applications."
        }
        (TextKey::CopyLastReplyEmpty, Locale::Ja) => "コピーできる返事がありません",
        (TextKey::CopyLastReplyEmpty, Locale::En) => "There is no reply to copy.",
        (TextKey::CopyLastReplyFailed, Locale::Ja) => "返事をコピーできませんでした: {error}",
        (TextKey::CopyLastReplyFailed, Locale::En) => "Could not copy the reply: {error}",
        (TextKey::LaunchAtLoginSyncFailed, Locale::Ja) => {
            "ログイン時起動を変更できませんでした: {error}"
        }
        (TextKey::LaunchAtLoginSyncFailed, Locale::En) => {
            "Could not change launch-at-login: {error}"
        }
        (TextKey::RuntimeClosed, Locale::Ja) => "runtime が停止しています",
        (TextKey::RuntimeClosed, Locale::En) => "The runtime is stopped.",
        (TextKey::RuntimeObservationCancelled, Locale::Ja) => "利用者入力で観察を中断しました",
        (TextKey::RuntimeObservationCancelled, Locale::En) => "Observation interrupted by user input",
        (TextKey::RuntimeConfigUpdateCancelled, Locale::Ja) => {
            "設定反映に伴い provider の呼び出しをキャンセルしました"
        }
        (TextKey::RuntimeConfigUpdateCancelled, Locale::En) => {
            "The provider call was cancelled while applying settings."
        }
        (TextKey::RuntimeObserverUnavailable, Locale::Ja) => "observer が設定されていません",
        (TextKey::RuntimeObserverUnavailable, Locale::En) => "The observer is not configured.",
        (TextKey::RuntimeCompanionUnavailable, Locale::Ja) => "companion が設定されていません",
        (TextKey::RuntimeCompanionUnavailable, Locale::En) => "The companion is not configured.",
        (TextKey::RuntimeResponseDropped, Locale::Ja) => "runtime 応答を受け取れませんでした",
        (TextKey::RuntimeResponseDropped, Locale::En) => "The runtime response was not received.",
        (TextKey::RuntimeProviderStartsBlocked, Locale::Ja) => {
            "設定更新中のため provider 操作を開始できません"
        }
        (TextKey::RuntimeProviderStartsBlocked, Locale::En) => {
            "Provider operations cannot start while settings are being updated."
        }
        (TextKey::RuntimeStaleWatchScope, Locale::Ja) => {
            "見守り対象の変更前に取得した frame です"
        }
        (TextKey::RuntimeStaleWatchScope, Locale::En) => {
            "This frame was captured before the watch target changed."
        }
        (TextKey::RuntimeObserverError, Locale::Ja) => "observer エラー: {error}",
        (TextKey::RuntimeObserverError, Locale::En) => "Observer error: {error}",
        (TextKey::RuntimeCompanionError, Locale::Ja) => "companion エラー: {error}",
        (TextKey::RuntimeCompanionError, Locale::En) => "Companion error: {error}",
        (TextKey::RuntimeConfigInvalid, Locale::Ja) => "runtime の設定が不正です: {error}",
        (TextKey::RuntimeConfigInvalid, Locale::En) => "The runtime configuration is invalid: {error}",
        (TextKey::ConfigUpdateFailed, Locale::Ja) => "設定の適用に失敗しました: {error}",
        (TextKey::ConfigUpdateFailed, Locale::En) => "Could not apply settings: {error}",
        (TextKey::RuntimeFactoryError, Locale::Ja) => "runtime の構成を作成できません: {error}",
        (TextKey::RuntimeFactoryError, Locale::En) => "Could not create the runtime: {error}",
        (TextKey::RuntimeCancelableReplyMissing, Locale::Ja) => "取り消せる返事はありません",
        (TextKey::RuntimeCancelableReplyMissing, Locale::En) => "There is no reply to cancel.",
        (TextKey::RuntimeRetryableReplyMissing, Locale::Ja) => "再試行できる発言はありません",
        (TextKey::RuntimeRetryableReplyMissing, Locale::En) => "There is no message to retry.",
        (TextKey::RuntimeMemoryMaintenanceUnavailable, Locale::Ja) => {
            "記憶メンテナンスは常駐 runtime でのみ利用できます"
        }
        (TextKey::RuntimeMemoryMaintenanceUnavailable, Locale::En) => {
            "Memory maintenance is available only in the resident runtime."
        }
        (TextKey::RuntimeFactoryRebuildUnavailable, Locale::Ja) => {
            "実行中の操作を設定変更後に再構築する RuntimeFactory がありません"
        }
        (TextKey::RuntimeFactoryRebuildUnavailable, Locale::En) => {
            "A RuntimeFactory is unavailable to rebuild the running operation after the settings change."
        }
        (TextKey::VoiceOutputDisabled, Locale::Ja) => "読み上げを有効にして反映してください",
        (TextKey::VoiceOutputDisabled, Locale::En) => "Enable voice output and apply the setting.",
        (TextKey::VoiceOutputTextTooLong, Locale::Ja) => {
            "読み上げる文章の長さが上限を超えています"
        }
        (TextKey::VoiceOutputTextTooLong, Locale::En) => {
            "The text for voice output exceeds the length limit."
        }
        (TextKey::VoiceOutputPending, Locale::Ja) => {
            "読み上げ待ちがあるため、この返答の読み上げを見送りました"
        }
        (TextKey::VoiceOutputPending, Locale::En) => {
            "This reply was not read aloud because another reply is waiting."
        }
        (TextKey::VoiceOutputInvalidInput, Locale::Ja) => {
            "読み上げる文章または速度が不正です"
        }
        (TextKey::VoiceOutputInvalidInput, Locale::En) => {
            "The voice-output text or speed is invalid."
        }
        (TextKey::VoiceOutputFailed, Locale::Ja) => {
            "macOS 標準音声で読み上げできませんでした。システムの声と音声出力を確認してください"
        }
        (TextKey::VoiceOutputFailed, Locale::En) => {
            "macOS standard voice output failed. Check the system voice and audio output."
        }
        (TextKey::VoicevoxEngineFailed, Locale::Ja) => "VOICEVOX ENGINE と通信できませんでした。ローカルのエンジンが起動していることを確認してください",
        (TextKey::VoicevoxEngineFailed, Locale::En) => "Could not communicate with VOICEVOX ENGINE. Make sure the local engine is running.",
        (TextKey::VoicevoxInvalidResponse, Locale::Ja) => "VOICEVOX ENGINE の応答が不正です",
        (TextKey::VoicevoxInvalidResponse, Locale::En) => "VOICEVOX ENGINE returned an invalid response.",
        (TextKey::VoicevoxFileFailed, Locale::Ja) => "VOICEVOX の一時音声ファイルを準備または削除できませんでした",
        (TextKey::VoicevoxFileFailed, Locale::En) => "Could not prepare or remove the temporary VOICEVOX audio file.",
        (TextKey::VoicevoxListCancelled, Locale::Ja) => "VOICEVOX の音声一覧取得を中止しました",
        (TextKey::VoicevoxListCancelled, Locale::En) => "VOICEVOX voice discovery was cancelled.",
        (TextKey::VoicevoxInvalidStyle, Locale::Ja) => "VOICEVOX の音声設定が不正です",
        (TextKey::VoicevoxInvalidStyle, Locale::En) => "The VOICEVOX voice setting is invalid.",
        (TextKey::VoicevoxPlaybackFailed, Locale::Ja) => "VOICEVOX の音声を再生できませんでした",
        (TextKey::VoicevoxPlaybackFailed, Locale::En) => "Could not play the VOICEVOX audio.",
        (TextKey::VoicevoxSelectVoice, Locale::Ja) => "VOICEVOX の声を選んでください。",
        (TextKey::VoicevoxSelectVoice, Locale::En) => "Select a VOICEVOX voice.",
        (TextKey::VoicevoxUnsupportedProvider, Locale::Ja) => "読み上げプロバイダーに対応していません",
        (TextKey::VoicevoxUnsupportedProvider, Locale::En) => "This voice-output provider is not supported.",
        (TextKey::VoicevoxUnsupportedLink, Locale::Ja) => "対応していないリンクです",
        (TextKey::VoicevoxUnsupportedLink, Locale::En) => "This link is not supported.",
        (TextKey::VoicevoxOpenLinkFailed, Locale::Ja) => "VOICEVOX の公式ページを開けませんでした",
        (TextKey::VoicevoxOpenLinkFailed, Locale::En) => "Could not open the official VOICEVOX page.",
        (TextKey::ConfigVoicevoxStyleRange, Locale::Ja) => "0以上2147483647以下の整数または null で指定してください。",
        (TextKey::ConfigVoicevoxStyleRange, Locale::En) => "Specify an integer from 0 to 2147483647, or null.",
        (TextKey::VoiceOutputTestText, Locale::Ja) => {
            "こんにちは。選んだ声で読み上げています。"
        }
        (TextKey::VoiceOutputTestText, Locale::En) => {
            "Hello. I am speaking with your selected voice."
        }
    }
}

pub fn localize_capture_message(message: &str, locale: Locale) -> String {
    let key = match message {
        message if matches_text(message, TextKey::CaptureAccessibilityRequired) => {
            Some(TextKey::CaptureAccessibilityRequired)
        }
        message if matches_text(message, TextKey::CaptureShortcutDisabled) => {
            Some(TextKey::CaptureShortcutDisabled)
        }
        "範囲選択を送信中です" | "The selection is being sent." => {
            Some(TextKey::CaptureSendInProgress)
        }
        "選択した文章を取得できませんでした" | "Could not get the selected text." => {
            Some(TextKey::CaptureSelectedTextFailed)
        }
        "選択した文章をコピーできませんでした" | "Could not copy the selected text." => {
            Some(TextKey::CaptureSelectedTextCopyFailed)
        }
        "クリップボードを読み取れませんでした" | "Could not read the clipboard." => {
            Some(TextKey::CaptureClipboardReadFailed)
        }
        "選択した画像を読み込めませんでした" | "Could not read the selected image." => {
            Some(TextKey::CaptureImageReadFailed)
        }
        "送信する範囲選択がありません" | "There is no selection to send." => {
            Some(TextKey::CaptureNoRegion)
        }
        "選択画像を読み込めません" | "Could not load the selected image." => {
            Some(TextKey::CaptureImageLoadFailed)
        }
        "送信する範囲選択が一致しません" | "The selection to send no longer matches." => {
            Some(TextKey::CaptureRegionMismatch)
        }
        "送信できる文章がありません" | "There is no text to send." => {
            Some(TextKey::CaptureTextUnavailable)
        }
        _ => None,
    };
    key.map_or_else(
        || {
            if locale == Locale::En && contains_japanese(message) {
                text(TextKey::CaptureOperationFailed, locale).to_owned()
            } else {
                message.to_owned()
            }
        },
        |key| text(key, locale).to_owned(),
    )
}

pub fn localize_audio_message(kind: &str, message: &str, locale: Locale) -> String {
    let key = match kind {
        "system-audio-permission" => Some(TextKey::AudioSystemPermissionRequired),
        "system-audio" => Some(TextKey::AudioSystemFailed),
        "system-audio-device" => Some(TextKey::AudioSystemDeviceUnavailable),
        "system-audio-format" => Some(TextKey::AudioSystemFormatFailed),
        "system-audio-overflow" => Some(TextKey::AudioSystemOverflow),
        "system-audio-start-timeout" => Some(TextKey::AudioSystemStartupTimeout),

        "input-device-fallback"
            if matches_text(message, TextKey::SpeechInputDeviceFallbackShort) =>
        {
            Some(TextKey::SpeechInputDeviceFallbackShort)
        }
        "input-device-fallback" if matches_text(message, TextKey::SpeechInputDeviceFallback) => {
            Some(TextKey::SpeechInputDeviceFallback)
        }
        "input-device-list" if matches_text(message, TextKey::SpeechInputDeviceListFallback) => {
            Some(TextKey::SpeechInputDeviceListFallback)
        }
        "input-device-list" if matches_text(message, TextKey::SpeechInputDeviceListFailed) => {
            Some(TextKey::SpeechInputDeviceListFailed)
        }
        "key-state" if matches_text(message, TextKey::SpeechKeyStateFailed) => {
            Some(TextKey::SpeechKeyStateFailed)
        }
        "permission-microphone" if matches_text(message, TextKey::SpeechMicrophoneDenied) => {
            Some(TextKey::SpeechMicrophoneDenied)
        }
        "permission-speech" if matches_text(message, TextKey::SpeechRecognitionDenied) => {
            Some(TextKey::SpeechRecognitionDenied)
        }
        "screen-capture" if matches_text(message, TextKey::AudioScreenPermissionRequired) => {
            Some(TextKey::AudioScreenPermissionRequired)
        }
        "input-source" if matches_text(message, TextKey::AudioSourceRequired) => {
            Some(TextKey::AudioSourceRequired)
        }
        "helper-unavailable" if matches_text(message, TextKey::AudioHelperMissing) => {
            Some(TextKey::AudioHelperMissing)
        }
        "helper-closed" if matches_text(message, TextKey::AudioHelperUnexpectedExit) => {
            Some(TextKey::AudioHelperUnexpectedExit)
        }
        "observation" if matches_text(message, TextKey::AudioObservationSaveFailed) => {
            Some(TextKey::AudioObservationSaveFailed)
        }
        "audio-ingestion" | "audio-delivery"
            if matches_text(message, TextKey::AudioWorkerStopped) =>
        {
            Some(TextKey::AudioWorkerStopped)
        }
        _ if matches_text(message, TextKey::AudioWarningGeneric) => {
            Some(TextKey::AudioWarningGeneric)
        }
        _ => None,
    };
    key.map_or_else(
        || {
            if locale == Locale::En && contains_japanese(message) {
                text(TextKey::AudioWarningGeneric, locale).to_owned()
            } else {
                message.to_owned()
            }
        },
        |key| text(key, locale).to_owned(),
    )
}

pub fn localize_speech_message(message: &str, locale: Locale) -> String {
    const KEYS: &[TextKey] = &[
        TextKey::SpeechSourceInvalid,
        TextKey::SpeechTextEmpty,
        TextKey::SpeechTextTooLong,
        TextKey::SpeechConfirmationMissing,
        TextKey::SpeechStaleInput,
        TextKey::SpeechRecognitionSettingsFailed,
        TextKey::SpeechMicrophoneSettingsFailed,
        TextKey::SpeechSettingsKindInvalid,
        TextKey::SpeechSending,
        TextKey::SpeechEnding,
        TextKey::SpeechGenerationMissing,
        TextKey::SpeechStartFailed,
        TextKey::SpeechInputEmpty,
        TextKey::SpeechHelperMissing,
        TextKey::SpeechNoSpeech,
        TextKey::SpeechGenericFailure,
        TextKey::SpeechMicrophoneDenied,
        TextKey::SpeechMicrophoneRestricted,
        TextKey::SpeechMicrophoneUnavailable,
        TextKey::SpeechRecognitionDenied,
        TextKey::SpeechRecognitionRestricted,
        TextKey::SpeechRecognitionUnavailable,
        TextKey::SpeechLocaleUnavailable,
        TextKey::SpeechOnDeviceUnsupported,
        TextKey::SpeechInputDeviceUnavailable,
        TextKey::SpeechInputDeviceListFailed,
        TextKey::SpeechInputDeviceListFallback,
        TextKey::SpeechKeyStateFailed,
        TextKey::SpeechInputDeviceFallback,
        TextKey::SpeechInputDeviceFallbackShort,
    ];
    KEYS.iter()
        .copied()
        .find(|&key| matches_text(message, key))
        .map_or_else(|| message.to_owned(), |key| text(key, locale).to_owned())
}

fn matches_text(message: &str, key: TextKey) -> bool {
    message == text(key, Locale::Ja) || message == text(key, Locale::En)
}

pub fn localize_error_message(message: &str, key: TextKey, locale: Locale) -> String {
    if message == text(key, Locale::Ja) || message == text(key, Locale::En) {
        text(key, locale).to_owned()
    } else if locale == Locale::Ja {
        message.to_owned()
    } else {
        text(key, locale).to_owned()
    }
}

pub fn localize_screen_permission_message(message: &str, locale: Locale) -> String {
    let key = match message {
        "システム設定の画面収録で CooSenpAI を許可して、アプリを再起動してください"
        | "Allow CooSenpAI under Screen Recording in System Settings, then restart the app." => {
            Some(TextKey::ScreenPermissionAllow)
        }
        "画面収録は許可済みですが、反映にはアプリの再起動が必要です"
        | "Screen Recording is allowed, but the app must be restarted for it to take effect." => {
            Some(TextKey::ScreenPermissionRestart)
        }
        "この Mac の制限により画面収録を利用できません"
        | "Screen Recording is unavailable because of restrictions on this Mac." => {
            Some(TextKey::ScreenPermissionRestricted)
        }
        "画面収録の権限状態を確認できません"
        | "Could not determine the Screen Recording permission status." => {
            Some(TextKey::ScreenPermissionUnavailable)
        }
        _ => None,
    };
    key.map_or_else(|| message.to_owned(), |key| text(key, locale).to_owned())
}

pub fn localize_capture_disposition(message: &str, locale: Locale) -> String {
    let key = match message {
        "撮影" | "Captured" => Some(TextKey::CaptureDispositionAccepted),
        "見送り（画面に変化なし）" | "Skipped (no screen change)" => {
            Some(TextKey::CaptureDispositionUnchanged)
        }
        "見送り（対象が無効です）" | "Skipped (target is disabled)" => {
            Some(TextKey::CaptureDispositionSuppressed)
        }
        "見送り（CooSenpAI が前面）" | "Skipped (CooSenpAI is in the foreground)" => {
            Some(TextKey::CaptureDispositionSelfApplication)
        }
        "見送り（自ウィンドウの範囲を取得できません）"
        | "Skipped (could not get the app window bounds)" => {
            Some(TextKey::CaptureDispositionOwnBoundsUnavailable)
        }
        "見送り（対象アプリのウィンドウがありません）"
        | "Skipped (the target app has no window)" => {
            Some(TextKey::CaptureDispositionWindowUnavailable)
        }
        "見送り（撮影間隔が短すぎます）" | "Skipped (capture interval is too short)" => {
            Some(TextKey::CaptureDispositionMinSpacing)
        }
        _ => None,
    };
    key.map_or_else(|| message.to_owned(), |key| text(key, locale).to_owned())
}

pub fn localize_shortcut_message(message: &str, locale: Locale) -> String {
    let parsed = [
        (
            "ショートカット ",
            " を解釈できません。",
            "Could not parse shortcut ",
            ".",
            TextKey::ShortcutInvalid,
        ),
        (
            "ショートカット ",
            " が別の操作と重複しています。",
            "Shortcut ",
            " is already assigned to another action.",
            TextKey::ShortcutDuplicate,
        ),
        (
            "ショートカット ",
            " は登録できません。別のキーを設定してください。",
            "Could not register shortcut ",
            ". Choose another key.",
            TextKey::ShortcutRegisterFailed,
        ),
    ]
    .into_iter()
    .find_map(|(ja_prefix, ja_suffix, en_prefix, en_suffix, key)| {
        message
            .strip_prefix(ja_prefix)
            .and_then(|value| value.strip_suffix(ja_suffix))
            .or_else(|| {
                message
                    .strip_prefix(en_prefix)
                    .and_then(|value| value.strip_suffix(en_suffix))
            })
            .map(|shortcut| (key, shortcut))
    });
    if let Some((key, shortcut)) = parsed {
        return text(key, locale).replace("{shortcut}", shortcut);
    }

    if let Some((shortcut, previous)) =
        message
            .split_once(" の登録に失敗し、以前の ")
            .and_then(|(shortcut, previous)| {
                previous
                    .strip_suffix(" も復元できませんでした")
                    .map(|previous| (shortcut, previous))
            })
    {
        return text(TextKey::ShortcutRestoreFailed, locale)
            .replace("{shortcut}", shortcut)
            .replace("{previous}", previous);
    }
    if let Some((shortcut, previous)) = message
        .strip_prefix("Could not register ")
        .and_then(|value| value.split_once("; the previous shortcuts ("))
        .and_then(|(shortcut, previous)| {
            previous
                .strip_suffix(") could not be restored.")
                .map(|previous| (shortcut, previous))
        })
    {
        return text(TextKey::ShortcutRestoreFailed, locale)
            .replace("{shortcut}", shortcut)
            .replace("{previous}", previous);
    }

    if locale == Locale::En && contains_japanese(message) {
        text(TextKey::ShortcutRegisterFailed, locale).replace("{shortcut}", "the shortcut")
    } else {
        message.to_owned()
    }
}

fn contains_japanese(message: &str) -> bool {
    message.chars().any(|character| {
        matches!(
            character,
            '\u{3040}'..='\u{30ff}' | '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}'
        )
    })
}

pub fn localize_onboarding_message(message: &str, locale: Locale) -> String {
    if locale == Locale::Ja {
        return message.to_owned();
    }
    let key = match message {
        "language が不正です" => Some(TextKey::SetupLanguageInvalid),
        "初回セットアップの言語選択を受け付けられません" => {
            Some(TextKey::SetupLanguageSelectionUnavailable)
        }
        "provider が不正です" => Some(TextKey::SetupProviderInvalid),
        "初回セットアップの provider 選択を受け付けられません" => {
            Some(TextKey::SetupProviderSelectionUnavailable)
        }
        "初回セットアップの接続方法選択を受け付けられません" => {
            Some(TextKey::SetupConnectionMethodSelectionUnavailable)
        }
        "初回セットアップの応答を受け付けられません" => {
            Some(TextKey::SetupResponseUnavailable)
        }
        "終了処理をやり直してください" => Some(TextKey::CommandTutorialFinishing),
        "設定画面の表示確認を受け付けられません" => {
            Some(TextKey::TutorialSettingsPresentationUnavailable)
        }
        _ => None,
    };
    key.map_or_else(|| message.to_owned(), |key| text(key, locale).to_owned())
}

pub fn localize_factory_message(message: &str, locale: Locale) -> String {
    if locale == Locale::Ja {
        return message.to_owned();
    }
    let key = match message {
        "実行ファイルのディレクトリを取得できません" => {
            Some(TextKey::FactoryExecutableDirectoryUnavailable)
        }
        "API キーを確認できません" => Some(TextKey::FactoryApiKeyCheckFailed),
        "API キーを読み込めません" => Some(TextKey::FactoryApiKeyReadFailed),
        "API キーを保存できません" => Some(TextKey::FactoryApiKeySaveFailed),
        "API キーを削除できません" => Some(TextKey::FactoryApiKeyDeleteFailed),
        "provider の認証設定を反映できません" => Some(TextKey::FactoryAuthApplyFailed),
        "provider の認証設定を復元できません" => {
            Some(TextKey::FactoryAuthRestoreFailed)
        }
        "provider bridge が見つかりません" => Some(TextKey::FactoryBridgeMissing),
        "Node.js 18 以上が見つかりません" => Some(TextKey::FactoryNodeMissing),
        "provider bridge を初期化できません" => Some(TextKey::FactoryBridgeInitFailed),
        "チュートリアル台本が見つかりません" => {
            Some(TextKey::FactoryTutorialMissing)
        }
        "英語のチュートリアル台本が見つかりません" => {
            Some(TextKey::FactoryEnglishTutorialMissing)
        }
        "終了処理中です" => Some(TextKey::FactoryShutdown),
        "provider のモデル情報がありません" => Some(TextKey::FactoryModelInfoMissing),
        "接続確認の応答が空でした" => Some(TextKey::FactoryConnectionEmpty),
        "組み込み persona の場所がありません" => {
            Some(TextKey::FactoryBuiltinPersonaMissing)
        }
        "取り消せる返事はありません" => Some(TextKey::RuntimeCancelableReplyMissing),
        "再試行できる発言はありません" => Some(TextKey::RuntimeRetryableReplyMissing),
        "記憶メンテナンスは常駐 runtime でのみ利用できます" => {
            Some(TextKey::RuntimeMemoryMaintenanceUnavailable)
        }
        "実行中の操作を設定変更後に再構築する RuntimeFactory がありません" => {
            Some(TextKey::RuntimeFactoryRebuildUnavailable)
        }
        _ => None,
    };
    if let Some(key) = key {
        return text(key, locale).to_owned();
    }
    if let Some(name) = message.strip_prefix("provider が不正です: ") {
        return text(TextKey::FactoryProviderInvalid, locale).replace("{name}", name);
    }
    if let Some(detail) = message.strip_prefix("persona を読み込めません: ") {
        return text(TextKey::FactoryPersonaLoadFailed, locale).replace("{detail}", detail);
    }
    if let Some((prefix, detail)) = message.split_once(": ") {
        let localized_detail = localize_factory_message(detail, locale);
        if localized_detail != detail {
            return format!("{prefix}: {localized_detail}");
        }
    }
    message.to_owned()
}

pub fn localize_config_issue_message(message: &str, locale: Locale) -> String {
    if locale == Locale::Ja {
        return message.to_owned();
    }
    let key = match message {
        "設定はオブジェクトで指定してください。" => {
            Some(TextKey::ConfigInvalidObject)
        }
        "配列で指定してください。" => Some(TextKey::ConfigInvalidArray),
        "未知のキーです。" => Some(TextKey::ConfigUnknownKey),
        "必須です。" => Some(TextKey::ConfigRequired),
        "正の整数で指定してください。" => Some(TextKey::ConfigPositiveInteger),
        "0以上の整数で指定してください。" => Some(TextKey::ConfigNonnegativeInteger),
        "0以上の整数または null で指定してください。" => {
            Some(TextKey::ConfigOptionalNonnegativeInteger)
        }
        "正の数で指定してください。" => Some(TextKey::ConfigPositiveNumber),
        "空でない文字列で指定してください。" => {
            Some(TextKey::ConfigNonemptyString)
        }
        "空でない文字列または null で指定してください。" => {
            Some(TextKey::ConfigNonemptyOrNullString)
        }
        "#RRGGBB 形式で指定してください。" => Some(TextKey::ConfigColor),
        "state 配下の png / jpg / jpeg ファイルパスで指定してください。" => {
            Some(TextKey::ConfigAvatarPath)
        }
        "空白以外の文字列で指定してください。" => {
            Some(TextKey::ConfigWhitespaceString)
        }
        "性格のIDが不正です。" => Some(TextKey::ConfigPersonaId),
        "実行ファイルは絶対パスまたは null で指定してください。" => {
            Some(TextKey::ConfigExecutable)
        }
        "typingPauseMs より小さくしてください。" => {
            Some(TextKey::ConfigActiveThreshold)
        }
        "pollMs 以上かつ maxIntervalMs 以下で指定してください。" => {
            Some(TextKey::ConfigSpacing)
        }
        "同じ bundle ID を重複して指定できません。"
        | "同じIDを複数の予定に指定できません。"
        | "同じショートカットを複数の操作に設定できません。" => {
            Some(TextKey::ConfigDuplicate)
        }
        "空でない制御文字を含まない bundle ID を指定してください。" => {
            Some(TextKey::ConfigBundleId)
        }
        "空でない制御文字を含まないアプリ名を指定してください。" => {
            Some(TextKey::ConfigAppName)
        }
        "1以上64以下のショートカット文字列または null で指定してください。" => {
            Some(TextKey::ConfigShortcut)
        }
        "Tauri が解釈できるショートカットを指定してください。" => {
            Some(TextKey::ConfigTauriShortcut)
        }
        "Escape は録音の取り消しに使うため設定できません。" => {
            Some(TextKey::ConfigEscapeShortcut)
        }
        "1以上20以下の制御文字を含まない名前で指定してください。" => {
            Some(TextKey::ConfigName)
        }
        "空文字または HH:MM 形式で指定してください。" => {
            Some(TextKey::ConfigReviewTime)
        }
        "10件以下で指定してください。" => Some(TextKey::ConfigReminderLimit),
        "1以上128以下の英数字とハイフンで指定してください。" => {
            Some(TextKey::ConfigReminderId)
        }
        "HH:MM 形式で指定してください。" => Some(TextKey::ConfigReminderTime),
        "1以上500以下の制御文字を含まない文字列で指定してください。" => {
            Some(TextKey::ConfigReminderTheme)
        }
        "ja または en で指定してください。" => Some(TextKey::ConfigLanguage),
        "system で指定してください。" | "system または voicevox で指定してください。" => {
            Some(TextKey::ConfigVoiceOutputProvider)
        }
        "VOICEVOX の声を選んでください。" => Some(TextKey::VoicevoxSelectVoice),
        "0以上2147483647以下の整数または null で指定してください。" => {
            Some(TextKey::ConfigVoicevoxStyleRange)
        }
        "OS 通知は署名済みビルドでのみ選択できます。" => {
            Some(TextKey::SettingsUnsignedBuildOnly)
        }
        "3 の整数で指定してください。" => Some(TextKey::ConfigVersionInteger),
        _ => None,
    };
    if let Some(key) = key {
        return text(key, locale).to_owned();
    }
    if let Some(values) = message.strip_suffix(" のいずれかで指定してください。") {
        return text(TextKey::ConfigAllowedValues, locale)
            .replace("{values}", &values.replace('、', ", "));
    }
    if let Some(values) = message.strip_suffix(" で指定してください。") {
        if let Some((first, second)) = values.split_once(" または ") {
            return text(TextKey::ConfigEither, locale)
                .replace("{first}", first)
                .replace("{second}", second);
        }
    }
    if let Some(minimum) = message
        .strip_suffix("以上の整数で指定してください。")
        .map(str::trim)
        .filter(|value| {
            !value.is_empty() && value.chars().all(|character| character.is_ascii_digit())
        })
    {
        return text(TextKey::ConfigPositiveRange, locale).replace("{minimum}", minimum);
    }
    if let Some(range) = message.strip_suffix("の整数で指定してください。") {
        if let Some((minimum, maximum)) = range.split_once("以上") {
            if let Some(maximum) = maximum.strip_suffix("以下") {
                if minimum.chars().all(|character| character.is_ascii_digit())
                    && maximum.chars().all(|character| character.is_ascii_digit())
                {
                    return text(TextKey::ConfigRange, locale)
                        .replace("{minimum}", minimum)
                        .replace("{maximum}", maximum);
                }
            }
        }
    }
    text(TextKey::ConfigInvalidValue, locale).to_owned()
}

