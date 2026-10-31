use super::*;
use iced::widget::image::Handle;
use std::collections::{HashMap, HashSet};
use video_management::thumbnails::{Outcome, Request, ResultEvent, Source, Worker};

const MEMORY_LIMIT: usize = 128;
const MEMORY_BYTES: usize = 16 * 1024 * 1024;

pub(super) enum State {
    Loading,
    Ready(Handle),
    Unavailable,
}

struct Entry {
    request: Request,
    state: State,
    bytes: usize,
    used: u64,
}

#[derive(Default)]
pub(super) struct Covers {
    worker: Option<Worker>,
    entries: HashMap<String, Entry>,
    // Failures are lightweight; remember them across image LRU eviction until sources change.
    failed: HashMap<String, Vec<Source>>,
    serial: u64,
    clock: u64,
    start_failed: bool,
}

impl Covers {
    pub fn state(&self, hash: &str) -> Option<&State> {
        self.entries.get(hash).map(|e| &e.state)
    }

    pub fn stop(&mut self) {
        if let Some(mut worker) = self.worker.take() {
            worker.shutdown();
        }
        self.start_failed = false;
        self.entries
            .retain(|_, entry| !matches!(entry.state, State::Loading));
    }

    fn receive(&mut self, event: ResultEvent) {
        let Some(entry) = self.entries.get_mut(&event.request.hash) else {
            return;
        };
        if entry.request != event.request {
            return;
        }
        match event.outcome {
            Outcome::Ready(image) => {
                entry.bytes = image.as_raw().len();
                entry.state = State::Ready(Handle::from_rgba(
                    image.width(),
                    image.height(),
                    image.into_raw(),
                ));
            }
            Outcome::NoFrame | Outcome::Failed(_) => {
                entry.bytes = 0;
                self.failed
                    .insert(event.request.hash, event.request.sources);
                entry.state = State::Unavailable;
            }
            Outcome::Cancelled => {
                self.entries.remove(&event.request.hash);
            }
        }
    }

    fn sync(
        &mut self,
        cache: PathBuf,
        desired: Vec<(String, Vec<Source>)>,
        live: &HashSet<String>,
        executor: Option<video_management::processing::Executor>,
    ) {
        self.clock += 1;
        self.failed.retain(|hash, _| live.contains(hash));
        self.entries.retain(|hash, _| live.contains(hash));
        let results: Vec<_> = self
            .worker
            .as_ref()
            .map(|w| w.results.try_iter().collect())
            .unwrap_or_default();
        for event in results {
            self.receive(event);
        }
        let wanted: HashSet<_> = desired.iter().map(|(hash, _)| hash.clone()).collect();
        self.entries
            .retain(|hash, entry| wanted.contains(hash) || !matches!(entry.state, State::Loading));
        let mut requests = Vec::new();
        for (hash, sources) in desired {
            if self.entries.get(&hash).is_some_and(|e| {
                e.request.sources != sources && !matches!(e.state, State::Ready(_))
            }) {
                self.entries.remove(&hash);
            }
            if !self.entries.contains_key(&hash) {
                self.serial += 1;
                let unavailable = self.start_failed || self.failed.get(&hash) == Some(&sources);
                self.entries.insert(
                    hash.clone(),
                    Entry {
                        request: Request {
                            id: self.serial,
                            hash: hash.clone(),
                            sources,
                        },
                        state: if unavailable {
                            State::Unavailable
                        } else {
                            State::Loading
                        },
                        bytes: 0,
                        used: self.clock,
                    },
                );
            }
            let entry = self.entries.get_mut(&hash).unwrap();
            entry.used = self.clock;
            if matches!(entry.state, State::Loading) {
                requests.push(entry.request.clone());
            }
        }
        // Visible covers are pinned (at most two 60-item pages).
        // Bound inactive image bytes too, rather than retaining 128 large images
        // after a user switches to a smaller page or closes the second window.
        let mut bytes: usize = self.entries.values().map(|entry| entry.bytes).sum();
        while self.entries.len() > MEMORY_LIMIT || bytes > MEMORY_BYTES {
            let oldest = self
                .entries
                .iter()
                .filter(|(h, _)| !wanted.contains(*h))
                .min_by_key(|(_, e)| e.used)
                .map(|(h, _)| h.clone());
            if let Some(hash) = oldest {
                bytes -= self.entries.remove(&hash).unwrap().bytes;
            } else {
                break;
            }
        }
        if self.worker.is_none()
            && !requests.is_empty()
            && !self.start_failed
            && let Some(executor) = executor
        {
            match Worker::start(cache, executor) {
                Ok(worker) => self.worker = Some(worker),
                Err(_) => {
                    self.start_failed = true;
                    for entry in self.entries.values_mut() {
                        if matches!(entry.state, State::Loading) {
                            entry.state = State::Unavailable;
                        }
                    }
                }
            }
        }
        if let Some(worker) = &self.worker {
            worker.request(requests);
        }
    }
}

impl App {
    pub(super) fn sync_covers(&mut self) {
        if self.updates.phase == updates::Phase::Stopping {
            return;
        }
        let mut videos = Vec::new();
        for pending in [false, true] {
            if (pending && self.pending.is_none()) || (!pending && self.page != MainPage::Library) {
                continue;
            }
            let list = self.filtered(pending);
            let range = self.pane(pending).pagination(list.len()).range();
            videos.extend_from_slice(&list[range]);
        }
        let mut seen = HashSet::new();
        let desired = videos
            .into_iter()
            .filter(|v| seen.insert(v.hash.clone()))
            .map(|v| {
                let sources = v
                    .files
                    .iter()
                    .filter(|f| {
                        f.availability == Availability::Present && f.state != FileState::Ignored
                    })
                    .map(|f| Source {
                        path: f.path.clone(),
                        size: f.size,
                        modified: f.modified,
                    })
                    .collect();
                (v.hash.clone(), sources)
            })
            .collect();
        let live = self
            .snapshot
            .videos
            .iter()
            .map(|v| v.hash.clone())
            .collect();
        self.covers.sync(
            self.data.join("cache/thumbnails"),
            desired,
            &live,
            self.service
                .as_ref()
                .filter(|_| self.app_settings_loaded)
                .map(|s| s.executor.clone()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_budget_evicts_old_images_but_preserves_visible_covers() {
        let mut covers = Covers::default();
        let live: HashSet<_> = (0..60).map(|id| format!("{id:064x}")).collect();
        for (id, hash) in live.iter().enumerate() {
            covers.entries.insert(
                hash.clone(),
                Entry {
                    request: Request {
                        id: id as u64,
                        hash: hash.clone(),
                        sources: vec![],
                    },
                    state: State::Loading,
                    bytes: 0,
                    used: id as u64,
                },
            );
            covers.receive(ResultEvent {
                request: covers.entries[hash].request.clone(),
                outcome: Outcome::Ready(image::RgbaImage::new(320, 320)),
            });
        }
        let all: Vec<_> = live.iter().map(|hash| (hash.clone(), vec![])).collect();
        covers.sync(PathBuf::new(), all, &live, None);
        assert_eq!(covers.entries.len(), 60, "visible images must remain ready");
        let visible = live.iter().next().unwrap().clone();
        covers.sync(PathBuf::new(), vec![(visible.clone(), vec![])], &live, None);
        assert!(matches!(covers.state(&visible), Some(State::Ready(_))));
        assert!(
            covers
                .entries
                .values()
                .map(|entry| entry.bytes)
                .sum::<usize>()
                <= MEMORY_BYTES
        );
        assert_eq!(covers.entries.len(), MEMORY_BYTES / (320 * 320 * 4));
    }

    #[test]
    fn stale_results_cannot_replace_or_cancel_a_new_attempt() {
        let hash = "b".repeat(64);
        let mut covers = Covers::default();
        let request = Request {
            id: 2,
            hash: hash.clone(),
            sources: vec![],
        };
        covers.entries.insert(
            hash.clone(),
            Entry {
                request: request.clone(),
                state: State::Loading,
                bytes: 0,
                used: 0,
            },
        );
        for outcome in [
            Outcome::Ready(image::RgbaImage::new(1, 1)),
            Outcome::Cancelled,
            Outcome::NoFrame,
        ] {
            covers.receive(ResultEvent {
                request: Request {
                    id: 1,
                    ..request.clone()
                },
                outcome,
            });
            assert!(matches!(covers.state(&hash), Some(State::Loading)));
        }
        covers.receive(ResultEvent {
            request,
            outcome: Outcome::Ready(image::RgbaImage::new(1, 1)),
        });
        assert!(matches!(covers.state(&hash), Some(State::Ready(_))));
        assert!(covers.failed.is_empty());
    }

    #[test]
    fn failures_survive_eviction_but_changed_sources_retry() {
        let mut covers = Covers {
            start_failed: true,
            ..Default::default()
        };
        let live = (0..140)
            .map(|i| format!("{i:064x}"))
            .collect::<HashSet<_>>();
        for hash in &live {
            covers.sync(PathBuf::new(), vec![(hash.clone(), vec![])], &live, None);
        }
        assert_eq!(covers.entries.len(), MEMORY_LIMIT);
        let hash = "a".repeat(64);
        let live = HashSet::from([hash.clone()]);
        covers.failed.insert(hash.clone(), vec![]);
        covers.start_failed = false;
        covers.sync(PathBuf::new(), vec![(hash.clone(), vec![])], &live, None);
        assert!(matches!(covers.state(&hash), Some(State::Unavailable)));
        assert!(covers.worker.is_none());
        // A source change replaces the failure entry with a new request generation.
        let previous = covers.entries[&hash].request.id;
        covers.start_failed = true;
        covers.sync(
            PathBuf::new(),
            vec![(
                hash.clone(),
                vec![Source {
                    path: "changed.mp4".into(),
                    size: 1,
                    modified: 2,
                }],
            )],
            &live,
            None,
        );
        assert!(covers.entries[&hash].request.id > previous);
    }
}
