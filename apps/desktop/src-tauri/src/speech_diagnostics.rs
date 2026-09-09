use super::{permission_name, SpeechSource};
use coosenpai_core::ports::{RuntimeLogger, SpeechPermissions};

pub(crate) enum SpeechStage {
    Starting(SpeechSource),
    PermissionRequest,
    Permissions(SpeechPermissions),
    HelperStart,
    Recording,
    FinalReceived { chars: usize },
    Confirming { chars: usize },
    Failed,
}

pub(crate) fn log_stage(logger: &dyn RuntimeLogger, generation: u64, stage: SpeechStage) {
    let detail = match stage {
        SpeechStage::Starting(source) => format!("starting source={}", source.as_str()),
        SpeechStage::PermissionRequest => "permission-request".to_owned(),
        SpeechStage::Permissions(permissions) => format!(
            "permissions microphone={} recognition={}",
            permission_name(permissions.microphone),
            permission_name(permissions.recognition),
        ),
        SpeechStage::HelperStart => "helper-start".to_owned(),
        SpeechStage::Recording => "recording".to_owned(),
        SpeechStage::FinalReceived { chars } => format!("final-received chars={chars}"),
        SpeechStage::Confirming { chars } => format!("confirming chars={chars}"),
        SpeechStage::Failed => "failed".to_owned(),
    };
    let _ = logger.write(
        "INFO",
        &format!("音声入力: generation={generation} stage={detail}"),
    );
}

