use super::{ConfigCommitError, ConfigUpdateTransaction};

pub(super) async fn run_after_best_effort<
    Display,
    Cleanup,
    DisplayOutput,
    DisplayError,
    CleanupOutput,
>(
    display: Display,
    cleanup: Cleanup,
) -> (Result<DisplayOutput, DisplayError>, CleanupOutput)
where
    Display: std::future::Future<Output = Result<DisplayOutput, DisplayError>>,
    Cleanup: std::future::Future<Output = CleanupOutput>,
{
    let display = display.await;
    (display, cleanup.await)
}

pub(super) async fn activate_production_then_complete<
    Agent,
    Build,
    BuildFuture,
    Replace,
    ReplaceFuture,
    HandleActivationFailure,
    HandleActivationFailureFuture,
    Complete,
    CompleteFuture,
>(
    production_restored: bool,
    build: Build,
    replace: Replace,
    handle_activation_failure: HandleActivationFailure,
    complete: Complete,
) -> Result<(), TutorialCompletionFailure>
where
    Build: FnOnce() -> BuildFuture,
    BuildFuture: std::future::Future<Output = Result<Agent, ConfigCommitError>>,
    Replace: FnOnce(Agent) -> ReplaceFuture,
    ReplaceFuture: std::future::Future<Output = Result<(), ConfigCommitError>>,
    HandleActivationFailure: FnOnce(ConfigCommitError) -> HandleActivationFailureFuture,
    HandleActivationFailureFuture: std::future::Future<Output = Result<(), ConfigCommitError>>,
    Complete: FnOnce() -> CompleteFuture,
    CompleteFuture: std::future::Future<Output = Result<(), ConfigCommitError>>,
{
    let activation_error = if !production_restored {
        match build().await {
            Ok(agents) => replace(agents).await.err(),
            Err(error) => Some(error),
        }
    } else {
        None
    };
    if let Some(error) = activation_error {
        handle_activation_failure(error)
            .await
            .map_err(TutorialCompletionFailure::Activation)?;
    }
    complete()
        .await
        .map_err(TutorialCompletionFailure::Persistence)
}

#[derive(Debug)]
pub(super) enum TutorialCompletionFailure {
    Activation(ConfigCommitError),
    Persistence(ConfigCommitError),
}

pub(super) async fn run_tutorial_finish_once<Active, ActiveFuture, Finish, FinishFuture>(
    transaction: ConfigUpdateTransaction<'_>,
    active: Active,
    finish: Finish,
) -> Result<(), ConfigCommitError>
where
    Active: FnOnce() -> ActiveFuture,
    ActiveFuture: std::future::Future<Output = bool>,
    Finish: FnOnce() -> FinishFuture,
    FinishFuture: std::future::Future<Output = Result<(), ConfigCommitError>>,
{
    if !active().await {
        return Ok(());
    }
    finish().await?;
    transaction.commit()
}
