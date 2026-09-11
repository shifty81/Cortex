use forge_core::ProjectSession;
use forge_process::{spawn, OperationEvent};
use forge_state::{ForgeStateStore, QueueRecord, QueueState};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::mpsc;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerResult {
    pub queue_id: String,
    pub project_id: String,
    pub operation: String,
    pub state: QueueState,
    pub operation_id: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SchedulerPolicy {
    pub max_items_per_run: usize,
    pub max_parallel_jobs: usize,
    pub max_parallel_mutations: usize,
    pub one_mutation_per_project: bool,
}

impl Default for SchedulerPolicy {
    fn default() -> Self {
        Self {
            max_items_per_run: 32,
            max_parallel_jobs: 1,
            max_parallel_mutations: 1,
            one_mutation_per_project: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueuePlanRow {
    pub queue_id: String,
    pub project_id: String,
    pub operation: String,
    pub mutates: bool,
    pub runnable: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SchedulerPlan {
    pub selected: Vec<QueuePlanRow>,
    pub deferred: Vec<QueuePlanRow>,
    pub effective_parallel_limit: usize,
}

pub struct ForgeScheduler;

impl ForgeScheduler {
    pub fn plan(store: &ForgeStateStore, policy: &SchedulerPolicy) -> SchedulerPlan {
        let snapshot = store.snapshot();
        let max_parallel = policy.max_parallel_jobs.max(1);
        let max_mutations = policy.max_parallel_mutations.max(1);
        let mut selected = Vec::new();
        let mut deferred = Vec::new();
        let mut selected_projects = BTreeSet::new();
        let mut mutations = 0usize;

        for job in snapshot
            .queue
            .iter()
            .filter(|job| job.state == QueueState::Queued)
            .take(policy.max_items_per_run.max(1))
        {
            let row = match snapshot.projects.iter().find(|project| project.id == job.project_id) {
                None => QueuePlanRow {
                    queue_id: job.id.clone(),
                    project_id: job.project_id.clone(),
                    operation: job.operation.clone(),
                    mutates: true,
                    runnable: false,
                    reason: "project is no longer registered".to_owned(),
                },
                Some(project) => {
                    let mutates = ProjectSession::load(&project.root)
                        .ok()
                        .and_then(|session| {
                            session
                                .capabilities()
                                .operations
                                .into_iter()
                                .find(|operation| operation.key == job.operation)
                        })
                        .map(|operation| operation.mutates)
                        .unwrap_or(true);
                    let mut reason = String::new();
                    let mut runnable = true;
                    if selected.len() >= max_parallel {
                        runnable = false;
                        reason = "parallel job budget reached".to_owned();
                    } else if mutates && mutations >= max_mutations {
                        runnable = false;
                        reason = "parallel mutation budget reached".to_owned();
                    } else if mutates
                        && policy.one_mutation_per_project
                        && selected_projects.contains(&job.project_id)
                    {
                        runnable = false;
                        reason = "project already has a selected mutation".to_owned();
                    }
                    QueuePlanRow {
                        queue_id: job.id.clone(),
                        project_id: job.project_id.clone(),
                        operation: job.operation.clone(),
                        mutates,
                        runnable,
                        reason,
                    }
                }
            };

            if row.runnable {
                if row.mutates {
                    mutations += 1;
                    selected_projects.insert(row.project_id.clone());
                }
                selected.push(row);
            } else {
                deferred.push(row);
            }
        }

        SchedulerPlan {
            selected,
            deferred,
            effective_parallel_limit: max_parallel,
        }
    }

    pub fn run_next(store: &mut ForgeStateStore) -> Result<Option<SchedulerResult>, String> {
        let Some(job) = store.claim_next()? else {
            return Ok(None);
        };
        let result = Self::run_claimed(store, &job);
        match result {
            Ok(result) => Ok(Some(result)),
            Err(error) => {
                let _ = store.finish_queue_item(
                    &job.id,
                    QueueState::Failed,
                    None,
                    error.clone(),
                );
                Err(error)
            }
        }
    }

    pub fn run_until_empty(
        store: &mut ForgeStateStore,
        max_items: usize,
    ) -> Result<Vec<SchedulerResult>, String> {
        let mut results = Vec::new();
        for _ in 0..max_items {
            match Self::run_next(store)? {
                Some(result) => results.push(result),
                None => break,
            }
        }
        Ok(results)
    }

    pub fn run_planned_sequential(
        store: &mut ForgeStateStore,
        policy: &SchedulerPolicy,
    ) -> Result<Vec<SchedulerResult>, String> {
        let plan = Self::plan(store, policy);
        let mut results = Vec::new();
        // Execution intentionally remains sequential until shared state locking and
        // process ownership are compiler/test certified. The planner already exposes
        // the future safe parallel batch without pretending concurrency exists today.
        for _ in 0..plan.selected.len() {
            if let Some(result) = Self::run_next(store)? {
                results.push(result);
            }
        }
        Ok(results)
    }

    fn run_claimed(
        store: &mut ForgeStateStore,
        job: &QueueRecord,
    ) -> Result<SchedulerResult, String> {
        let root = store
            .snapshot()
            .projects
            .iter()
            .find(|project| project.id == job.project_id)
            .map(|project| project.root.clone())
            .ok_or_else(|| {
                format!(
                    "scheduled project is no longer registered: {}",
                    job.project_id
                )
            })?;
        let session = ProjectSession::load(&root).map_err(|error| error.to_string())?;
        let spec = session
            .resolve(&job.operation)
            .map_err(|error| error.to_string())?;
        let (tx, rx) = mpsc::channel();
        let handle = spawn(spec, tx)?;
        let operation_id = handle.id().to_owned();

        loop {
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(OperationEvent::Finished { success, code, .. }) => {
                    let state = if success {
                        QueueState::Succeeded
                    } else {
                        QueueState::Failed
                    };
                    let detail = format!(
                        "operation finished with exit {}",
                        code.map_or_else(|| "?".to_owned(), |value| value.to_string())
                    );
                    store.finish_queue_item(
                        &job.id,
                        state,
                        Some(operation_id.clone()),
                        detail.clone(),
                    )?;
                    return Ok(SchedulerResult {
                        queue_id: job.id.clone(),
                        project_id: job.project_id.clone(),
                        operation: job.operation.clone(),
                        state,
                        operation_id: Some(operation_id),
                        detail,
                    });
                }
                Ok(OperationEvent::Cancelled { .. }) => {
                    let detail = "scheduled operation cancelled".to_owned();
                    store.finish_queue_item(
                        &job.id,
                        QueueState::Cancelled,
                        Some(operation_id.clone()),
                        detail.clone(),
                    )?;
                    return Ok(SchedulerResult {
                        queue_id: job.id.clone(),
                        project_id: job.project_id.clone(),
                        operation: job.operation.clone(),
                        state: QueueState::Cancelled,
                        operation_id: Some(operation_id),
                        detail,
                    });
                }
                Ok(OperationEvent::FailedToStart { error, .. })
                | Ok(OperationEvent::HostError { error, .. }) => {
                    store.finish_queue_item(
                        &job.id,
                        QueueState::Failed,
                        Some(operation_id.clone()),
                        error.clone(),
                    )?;
                    return Ok(SchedulerResult {
                        queue_id: job.id.clone(),
                        project_id: job.project_id.clone(),
                        operation: job.operation.clone(),
                        state: QueueState::Failed,
                        operation_id: Some(operation_id),
                        detail: error,
                    });
                }
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let detail =
                        "operation event channel disconnected before terminal state".to_owned();
                    store.finish_queue_item(
                        &job.id,
                        QueueState::Interrupted,
                        Some(operation_id.clone()),
                        detail.clone(),
                    )?;
                    return Ok(SchedulerResult {
                        queue_id: job.id.clone(),
                        project_id: job.project_id.clone(),
                        operation: job.operation.clone(),
                        state: QueueState::Interrupted,
                        operation_id: Some(operation_id),
                        detail,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduler_result_serializes_terminal_state() {
        let value = serde_json::to_value(SchedulerResult {
            queue_id: "q".to_owned(),
            project_id: "p".to_owned(),
            operation: "gate.full".to_owned(),
            state: QueueState::Succeeded,
            operation_id: Some("op".to_owned()),
            detail: "done".to_owned(),
        })
        .expect("json");
        assert_eq!(value["state"], "succeeded");
    }

    #[test]
    fn default_policy_is_conservative() {
        let policy = SchedulerPolicy::default();
        assert_eq!(policy.max_parallel_jobs, 1);
        assert_eq!(policy.max_parallel_mutations, 1);
    }
}
