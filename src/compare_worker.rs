use crate::{
    compare::{self, CompareEndpoint, CompareReport},
    model::Credentials,
};
use anyhow::Result;
use std::sync::mpsc;
use uuid::Uuid;

// Requests own ephemeral credentials and endpoint snapshots until dispatched.
// Keep one sealed request payload rather than scattering secrets across tasks.
#[allow(clippy::large_enum_variant)]
pub enum CompareRequest {
    Compare {
        document: Uuid,
        generation: Uuid,
        source: CompareEndpoint,
        target: CompareEndpoint,
        source_credentials: Credentials,
        target_credentials: Credentials,
    },
    Apply {
        document: Uuid,
        generation: Uuid,
        report: Box<CompareReport>,
        target_credentials: Credentials,
    },
    ApplyStructure {
        document: Uuid,
        generation: Uuid,
        report: Box<CompareReport>,
        candidate: usize,
        source_credentials: Credentials,
        target_credentials: Credentials,
    },
}
#[derive(Clone, Debug)]
pub enum CompareResponseKind {
    Compared(Result<Box<CompareReport>, String>),
    Applied(Result<(usize, usize), String>),
    StructureApplied(Result<String, String>),
}
#[derive(Clone, Debug)]
pub struct CompareResponse {
    pub document: Uuid,
    pub generation: Uuid,
    pub source: String,
    pub target: String,
    pub kind: CompareResponseKind,
}
pub struct CompareWorker {
    sender: tokio::sync::mpsc::UnboundedSender<CompareRequest>,
    receiver: mpsc::Receiver<CompareResponse>,
}
impl CompareWorker {
    pub fn start() -> Self {
        let (sender, mut requests) = tokio::sync::mpsc::unbounded_channel::<CompareRequest>();
        let (responses, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            let local = tokio::task::LocalSet::new();
            local.block_on(&runtime, async move {
                while let Some(request) = requests.recv().await {
                    let (document, generation, source, target, kind) = match request {
                        CompareRequest::Compare {
                            document,
                            generation,
                            source,
                            target,
                            source_credentials,
                            target_credentials,
                        } => {
                            let result = compare::compare_tables(
                                &source,
                                &source_credentials,
                                &target,
                                &target_credentials,
                            )
                            .await
                            .map(Box::new)
                            .map_err(|e| {
                                redact(
                                    &format!("{e:#}"),
                                    &[&source_credentials, &target_credentials],
                                )
                            });
                            (
                                document,
                                generation,
                                source.fingerprint(),
                                target.fingerprint(),
                                CompareResponseKind::Compared(result),
                            )
                        }
                        CompareRequest::Apply {
                            document,
                            generation,
                            report,
                            target_credentials,
                        } => {
                            let result = compare::apply_sync(&report, &target_credentials)
                                .await
                                .map_err(|e| redact(&format!("{e:#}"), &[&target_credentials]));
                            (
                                document,
                                generation,
                                report.source.endpoint.fingerprint(),
                                report.target.endpoint.fingerprint(),
                                CompareResponseKind::Applied(result),
                            )
                        }
                        CompareRequest::ApplyStructure {
                            document,
                            generation,
                            report,
                            candidate,
                            source_credentials,
                            target_credentials,
                        } => {
                            let result = compare::apply_structure_sync(
                                &report,
                                candidate,
                                &source_credentials,
                                &target_credentials,
                            )
                            .await
                            .map_err(|e| {
                                redact(
                                    &format!("{e:#}"),
                                    &[&source_credentials, &target_credentials],
                                )
                            });
                            (
                                document,
                                generation,
                                report.source.endpoint.fingerprint(),
                                report.target.endpoint.fingerprint(),
                                CompareResponseKind::StructureApplied(result),
                            )
                        }
                    };
                    if responses
                        .send(CompareResponse {
                            document,
                            generation,
                            source,
                            target,
                            kind,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            });
        });
        Self { sender, receiver }
    }
    pub fn send(&self, request: CompareRequest) -> Result<()> {
        self.sender
            .send(request)
            .map_err(|_| anyhow::anyhow!("比较工作线程已停止，请重新打开比较页面"))
    }
    pub fn try_recv(&self) -> Option<CompareResponse> {
        self.receiver.try_recv().ok()
    }
}
fn redact(message: &str, credentials: &[&Credentials]) -> String {
    let mut message = message.to_owned();
    for credential in credentials {
        for secret in [&credential.database_password, &credential.ssh_secret] {
            if !secret.is_empty() {
                message = message.replace(secret, "[已隐去凭据]");
            }
        }
    }
    message.chars().take(8000).collect()
}
