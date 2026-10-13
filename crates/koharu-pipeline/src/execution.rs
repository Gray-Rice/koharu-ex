use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use anyhow::{Context as _, Result, ensure};
use chrono::{DateTime, Utc};
use futures::{StreamExt as _, stream::FuturesUnordered};
use koharu_scene::{EntityId, Snapshot};

use crate::{
    Committer, ErrorKind, PipelineError, Progress, ProgressSink, Request, Stage,
    StageOutput, StopToken,
    images::ImageCache,
    progress,
    resources::ResourceMonitor,
    scheduler::Scheduler,
    scope::NormalizedScope,
    stage_runner::{StageCompletion, StageJob, StageOutcome, StageRunner},
    stages::StageInput,
};

/// Error information for a single page that failed during pipeline execution.
#[derive(Debug, Clone)]
pub struct PageError {
    pub page: EntityId,
    pub stage: Stage,
    pub error: String,
    pub output: Option<String>,
    pub timestamp: DateTime<Utc>,
}

/// Result of pipeline execution containing both successful pages and collected errors.
#[derive(Debug, Clone)]
pub struct PipelineResult {
    pub successful_pages: Vec<EntityId>,
    pub errors: Vec<PageError>,
}

pub(crate) struct Execution<'a> {
    runner: Arc<StageRunner>,
    resources: Arc<ResourceMonitor>,
    committer: &'a mut dyn Committer,
    stop: StopToken,
    progress: Option<ProgressSink>,
    scope: NormalizedScope,
    scheduler: Scheduler,
    scene: Snapshot,
    images: BTreeMap<EntityId, Arc<ImageCache>>,
    busy_stages: BTreeSet<Stage>,
    completed: usize,
    page_errors: Vec<PageError>,
    inpainting_mask: Option<crate::InpaintingMask>,
}

impl<'a> Execution<'a> {
    pub(crate) fn new(
        runner: Arc<StageRunner>,
        resources: Arc<ResourceMonitor>,
        snapshot: Snapshot,
        request: Request,
        committer: &'a mut dyn Committer,
    ) -> std::result::Result<Self, PipelineError> {
        let stages = request
            .operation
            .stages()
            .map_err(|error| PipelineError::new(ErrorKind::InvalidInput, None, error))?;
        let scope = NormalizedScope::new(&snapshot, &request.scope, &stages)
            .map_err(|error| PipelineError::new(ErrorKind::InvalidInput, None, error))?;
        let pages = scope.pages().to_vec();
        if let Some(mask) = request.inpainting_mask.as_ref()
            && (!pages.contains(&mask.page) || !stages.contains(&Stage::Inpainting))
        {
            return Err(PipelineError::new(
                ErrorKind::InvalidInput,
                Some(Stage::Inpainting),
                anyhow::anyhow!("the inpainting mask page is outside the inpainting scope"),
            ));
        }
        progress::emit(
            request.progress.as_ref(),
            Progress::Started {
                pages: pages.clone(),
                stages: stages.clone(),
            },
        );

        Ok(Self {
            runner,
            resources,
            committer,
            stop: request.stop,
            progress: request.progress,
            scope,
            scheduler: Scheduler::new(&pages, &stages),
            scene: snapshot,
            images: BTreeMap::new(),
            busy_stages: BTreeSet::new(),
            completed: 0,
            page_errors: Vec::new(),
            inpainting_mask: request.inpainting_mask,
        })
    }

    pub(crate) async fn run(mut self) -> std::result::Result<PipelineResult, PipelineError> {
        if self.stopped() {
            let successful_pages = self.scheduler.successful_pages();
            return Ok(PipelineResult {
                successful_pages,
                errors: self.page_errors,
            });
        }

        self.resources.start();
        self.resources.wait_for_sample().await;

        let runner = self.runner.clone();
        let mut running = FuturesUnordered::new();
        loop {
            while let Some(job) = self.take_ready_job() {
                running.push(runner.run(job));
            }

            let Some(completion) = running.next().await else {
                break;
            };
            let page = completion.page;
            let stage = completion.stage;
            self.busy_stages.remove(&stage);
            if self.stopped() {
                continue;
            }
            if let Err(error) = self.apply_completion(completion).await {
                self.record_page_error(page, stage, error);
            }
        }

        self.finalize()
    }

    fn take_ready_job(&mut self) -> Option<StageJob> {
        if self.stopped() {
            return None;
        }
        let (page, stage) = self.scheduler.start_next(&self.busy_stages)?;
        self.busy_stages.insert(stage);
        let images = self
            .images
            .entry(page)
            .or_insert_with(|| Arc::new(ImageCache::default()))
            .clone();
        Some(StageJob::new(
            stage,
            StageInput::new(
                self.scene.clone(),
                page,
                self.scope.entities(),
                self.scope.region(page),
                images,
                self.inpainting_mask
                    .as_ref()
                    .filter(|mask| stage == Stage::Inpainting && mask.page == page)
                    .cloned(),
            ),
            self.stop.clone(),
            self.progress.clone(),
        ))
    }

    async fn apply_completion(
        &mut self,
        completion: StageCompletion,
    ) -> std::result::Result<(), PipelineError> {
        let StageCompletion {
            page,
            stage,
            model,
            elapsed,
            outcome,
        } = completion;
        match outcome? {
            StageOutcome::Stopped => {}
            StageOutcome::Skipped => {
                self.mark_complete(page, stage);
                progress::emit(self.progress.as_ref(), Progress::Skipped { page, stage });
            }
            StageOutcome::Patch(patch) => {
                if !self.commit_patch(page, stage, patch).await? {
                    return Ok(());
                }
                self.mark_complete(page, stage);
                progress::emit(
                    self.progress.as_ref(),
                    Progress::Finished {
                        page,
                        stage,
                        model,
                        elapsed,
                    },
                );
            }
        }
        Ok(())
    }

    fn record_page_error(&mut self, page: EntityId, stage: Stage, error: PipelineError) {
        let page_error = PageError {
            page,
            stage,
            error: error.to_string(),
            output: None,
            timestamp: Utc::now(),
        };

        progress::emit(
            self.progress.as_ref(),
            Progress::Failed {
                page: page_error.page,
                stage: page_error.stage,
                error: page_error.error.clone(),
                output: page_error.output.clone(),
            },
        );

        self.scheduler.fail_stage(page, stage);
        self.page_errors.push(page_error);
    }

    fn mark_complete(&mut self, page: EntityId, stage: Stage) {
        if self.scheduler.complete_stage(page, stage) {
            self.images.remove(&page);
        }
        self.completed += 1;
    }

    async fn commit_patch(
        &mut self,
        page: EntityId,
        stage: Stage,
        patch: koharu_scene::Patch,
    ) -> std::result::Result<bool, PipelineError> {
        let patch = patch
            .rebase_on(&self.scene)
            .and_then(|patch| {
                patch.validate_on(&self.scene)?;
                Ok(patch.with_label(format!("Pipeline {stage} for page {page}")))
            })
            .context("failed to rebase stage output onto the latest scene")
            .map_err(|error| PipelineError::new(ErrorKind::InvalidOutput, Some(stage), error))?;
        if self.stopped() {
            return Ok(false);
        }

        let next = self
            .committer
            .commit(StageOutput { page, stage, patch })
            .await
            .with_context(|| format!("failed to commit {stage} output for page {page}"))
            .map_err(|error| PipelineError::new(ErrorKind::Commit, Some(stage), error))?;
        validate_commit(&self.scene, &next)
            .map_err(|error| PipelineError::new(ErrorKind::Commit, Some(stage), error))?;
        self.scene = next;
        Ok(true)
    }

    fn stopped(&self) -> bool {
        self.stop.stopped()
    }

    // A stop is a partial success, not a failure: pages that finished before
    // the stop are reported, matching the early-return path for a stop that
    // happens before any work starts.
    fn finalize(self) -> std::result::Result<PipelineResult, PipelineError> {
        let successful_pages = self.scheduler.successful_pages();

        Ok(PipelineResult {
            successful_pages,
            errors: self.page_errors,
        })
    }
}

fn validate_commit(previous: &Snapshot, next: &Snapshot) -> Result<()> {
    ensure!(
        previous.project_id() == next.project_id(),
        "committer returned a snapshot from another project"
    );
    ensure!(
        next.revision() > previous.revision(),
        "committer did not advance the scene revision"
    );
    Ok(())
}
