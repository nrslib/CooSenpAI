use super::runtime::{ActivationLease, ActivationPort};
use super::{ActivationPolicy, OriginObservation};
use crate::capture::manager::tests::FakePort;
use crate::capture::{CaptureOrigin, ReadyCapture};
use coosenpai_core::ports::ForegroundApplication;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

type ReturnReply = oneshot::Receiver<Result<(), String>>;

#[derive(Default)]
pub(super) struct RecordingActivationPort {
    capture: Option<FakePort>,
    pub(super) return_gate: Mutex<Option<ReturnReply>>,
    pub(super) key_gate: Mutex<Option<oneshot::Receiver<()>>>,
}

pub(crate) fn capture_policy(
    capture: FakePort,
    origin: Option<ForegroundApplication>,
    return_gate: Option<ReturnReply>,
) -> ActivationPolicy {
    if let Some(origin) = &origin {
        let mut applications = capture.applications.lock().unwrap();
        applications.self_active = true;
        applications.frontmost = Some(applications.own_application());
        applications.running.push(origin.clone());
    }
    let mut policy = ActivationPolicy::with_port(RecordingActivationPort {
        capture: Some(capture),
        return_gate: Mutex::new(return_gate),
        ..Default::default()
    });
    policy.main_origin = origin;
    policy
}

impl Default for ActivationPolicy {
    fn default() -> Self {
        Self::with_port(RecordingActivationPort::default())
    }
}

#[async_trait::async_trait]
impl ActivationPort for RecordingActivationPort {
    fn log(&self, message: &str) {
        if let Some(capture) = &self.capture {
            capture.log(message);
        }
    }

    fn log_error(&self, message: &str) {
        if let Some(capture) = &self.capture {
            capture.log(&format!("ERROR {message}"));
        }
    }

    async fn observe_origin(
        &self,
        lease: ActivationLease,
        main_origin: Option<ForegroundApplication>,
    ) -> Result<OriginObservation, String> {
        lease.execute(|| {
            if let Some(capture) = &self.capture {
                capture.log("capture:prepare-origin");
            }
            let Some(capture) = &self.capture else {
                return Ok(OriginObservation::default());
            };
            let applications = capture.applications.lock().unwrap();
            Ok(OriginObservation {
                self_active: applications.self_active,
                self_pid: 1,
                frontmost: applications
                    .frontmost
                    .clone()
                    .filter(|app| applications.running.contains(app)),
                main_origin: main_origin.filter(|app| applications.running.contains(app)),
            })
        })
    }

    async fn return_to_origin(
        &self,
        lease: ActivationLease,
        origin: ForegroundApplication,
        expires_at: Option<tokio::time::Instant>,
    ) -> Result<(), String> {
        if let Some(capture) = &self.capture {
            capture.log("capture:return-origin:begin");
            let gate = capture.activation_queue_gate.lock().unwrap().take();
            let capture = capture.clone();
            let activation = lease.clone();
            if let Some(gate) = gate {
                let (reply, result) = oneshot::channel();
                // main-thread queue は呼出 future が取り消されても残る。その境界を再現する。
                tokio::spawn(async move {
                    gate.await.unwrap();
                    let result = activation.execute_before(expires_at, "origin timeout", || {
                        capture
                            .applications
                            .lock()
                            .unwrap()
                            .activate(origin.clone());
                        capture.log("capture:return-origin:activate");
                        Ok(())
                    });
                    capture.log("capture:return-origin:callback-finished");
                    let _ = reply.send(result);
                });
                result.await.unwrap()?;
            } else {
                activation.execute_before(expires_at, "origin timeout", || {
                    capture
                        .applications
                        .lock()
                        .unwrap()
                        .activate(origin.clone());
                    capture.log("capture:return-origin:activate");
                    Ok(())
                })?;
            }
        }
        let gate = self.return_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.await.unwrap()?;
        }
        lease.execute(|| {
            if let Some(capture) = &self.capture {
                capture.log("capture:return-origin:completed");
            }
            Ok(())
        })
    }

    async fn show_popup(
        &self,
        lease: ActivationLease,
        generation: u64,
        content: Arc<ReadyCapture>,
    ) -> Result<(), String> {
        lease.execute(|| Ok(()))?;
        if let Some(capture) = &self.capture {
            capture.show(generation, &content).await?;
            capture.applications.lock().unwrap().popup_key = true;
        }
        Ok(())
    }

    async fn reacquire_popup_key(
        &self,
        lease: ActivationLease,
        generation: u64,
        expires_at: Option<tokio::time::Instant>,
    ) -> Result<bool, String> {
        let gate = self.key_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.await
                .map_err(|_| "key notification closed".to_owned())?;
        }
        lease.execute_key_before(expires_at, || {
            if let Some(capture) = &self.capture {
                capture.reacquire_key(generation)?;
                let mut apps = capture.applications.lock().unwrap();
                apps.popup_key = apps.key_results.pop_front().unwrap_or(Ok(true))?;
                return Ok(apps.popup_key);
            }
            Ok(true)
        })
    }

    async fn restore_origin(
        &self,
        lease: ActivationLease,
        origin: CaptureOrigin,
    ) -> Result<(), String> {
        lease.execute(|| {
            if let Some(capture) = &self.capture {
                if let Some(application) = &origin.frontmost_application {
                    let mut applications = capture.applications.lock().unwrap();
                    if applications.running.contains(application) {
                        applications.activate(application.clone());
                    }
                }
                capture
                    .restored_origins
                    .lock()
                    .unwrap()
                    .push(origin.frontmost_application);
                capture.log("capture:restore-origin");
            }
            Ok(())
        })
    }
}

#[derive(Debug)]
pub(crate) struct RecordingApplications {
    pub(crate) frontmost: Option<ForegroundApplication>,
    pub(crate) self_active: bool,
    pub(crate) popup_key: bool,
    pub(crate) key_results: std::collections::VecDeque<Result<bool, String>>,
    pub(crate) running: Vec<ForegroundApplication>,
    pub(crate) activations: Vec<ForegroundApplication>,
}

impl Default for RecordingApplications {
    fn default() -> Self {
        Self {
            frontmost: None,
            self_active: false,
            popup_key: false,
            key_results: Default::default(),
            running: vec![ForegroundApplication {
                process_id: 1,
                bundle_id: None,
            }],
            activations: Vec::new(),
        }
    }
}

impl RecordingApplications {
    fn own_application(&self) -> ForegroundApplication {
        ForegroundApplication {
            process_id: 1,
            bundle_id: None,
        }
    }

    pub(crate) fn show_main(&mut self) {
        self.activate(self.own_application());
    }

    fn activate(&mut self, application: ForegroundApplication) {
        assert!(self.running.contains(&application));
        self.popup_key = false;
        self.self_active = application.process_id == 1;
        self.frontmost = Some(application.clone());
        self.activations.push(application);
    }
}

pub(crate) fn origin(application: Option<ForegroundApplication>) -> CaptureOrigin {
    CaptureOrigin {
        frontmost_application: application,
    }
}
