use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::de::DeserializeOwned;
use tokio::time::{Instant, sleep};

use super::{DEFAULT_TIMEOUT, POLL_INTERVAL, TestHarness};

#[derive(Clone)]
pub struct FsHandle {
    pub(super) harness: TestHarness,
    pub(super) root: PathBuf,
}

impl FsHandle {
    fn path(&self, rel: impl AsRef<Path>) -> PathBuf {
        let rel = rel.as_ref();
        if rel.is_absolute() {
            rel.to_path_buf()
        } else {
            self.root.join(rel)
        }
    }

    pub fn write_text(&self, rel: impl AsRef<Path>, contents: impl AsRef<str>) -> Result<()> {
        let path = self.path(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, contents.as_ref())?;
        Ok(())
    }

    pub fn append_text(&self, rel: impl AsRef<Path>, contents: impl AsRef<str>) -> Result<()> {
        let path = self.path(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        file.write_all(contents.as_ref().as_bytes())?;
        Ok(())
    }

    pub fn remove(&self, rel: impl AsRef<Path>) -> Result<()> {
        let path = self.path(rel);
        if path.is_dir() {
            std::fs::remove_dir_all(path)?;
        } else if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    pub fn assert_exists(&self, rel: impl AsRef<Path>) -> Result<()> {
        let path = self.path(rel);
        if path.exists() {
            Ok(())
        } else {
            Err(anyhow!("expected path to exist: {}", path.display()))
        }
    }

    pub fn assert_missing(&self, rel: impl AsRef<Path>) -> Result<()> {
        let path = self.path(rel);
        if !path.exists() {
            Ok(())
        } else {
            Err(anyhow!("expected path to be missing: {}", path.display()))
        }
    }

    pub fn read_text(&self, rel: impl AsRef<Path>) -> Result<String> {
        let path = self.path(rel);
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))
    }

    pub fn assert_file_contains(&self, rel: impl AsRef<Path>, needle: &str) -> Result<()> {
        let path = self.path(rel);
        let body =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        if body.contains(needle) {
            Ok(())
        } else {
            Err(anyhow!(
                "expected {} to contain {:?}\ncontents:\n{}",
                path.display(),
                needle,
                body,
            ))
        }
    }

    pub async fn wait_for_file_contains(
        &self,
        rel: impl AsRef<Path>,
        needle: &str,
    ) -> Result<()> {
        let path = self.path(rel);
        let needle = needle.to_string();
        match self
            .harness
            .wait_until(
                DEFAULT_TIMEOUT,
                format!("{} to contain {:?}", path.display(), needle),
                || {
                    let path = path.clone();
                    let needle = needle.clone();
                    async move {
                        if !path.exists() {
                            return Ok(None);
                        }
                        let body = std::fs::read_to_string(&path)?;
                        if body.contains(&needle) {
                            Ok(Some(()))
                        } else {
                            Ok(None)
                        }
                    }
                },
            )
            .await
        {
            Ok(()) => Ok(()),
            Err(err) => {
                let body = std::fs::read_to_string(&path).unwrap_or_default();
                Err(err.context(format!("current contents of {}:\n{}", path.display(), body)))
            }
        }
    }

    pub fn assert_json_file<T: DeserializeOwned>(&self, rel: impl AsRef<Path>) -> Result<T> {
        let path = self.path(rel);
        let body =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        serde_json::from_str(&body).with_context(|| format!("parse {} as json", path.display()))
    }

    pub fn line_count(&self, rel: impl AsRef<Path>) -> Result<usize> {
        let path = self.path(rel);
        if !path.exists() {
            return Ok(0);
        }
        let body = std::fs::read_to_string(path)?;
        Ok(body.lines().count())
    }

    pub async fn wait_for_line_count_at_least(
        &self,
        rel: impl AsRef<Path>,
        expected: usize,
    ) -> Result<()> {
        let path = self.path(rel);
        match self
            .harness
            .wait_until(
                DEFAULT_TIMEOUT,
                format!("{} to have at least {expected} lines", path.display()),
                || {
                    let path = path.clone();
                    async move {
                        if !path.exists() {
                            return Ok(None);
                        }
                        let body = std::fs::read_to_string(&path)?;
                        if body.lines().count() >= expected {
                            Ok(Some(()))
                        } else {
                            Ok(None)
                        }
                    }
                },
            )
            .await
        {
            Ok(()) => Ok(()),
            Err(err) => {
                let body = std::fs::read_to_string(&path).unwrap_or_default();
                Err(err.context(format!("current contents of {}:\n{}", path.display(), body)))
            }
        }
    }

    pub async fn assert_line_count_stays(
        &self,
        rel: impl AsRef<Path>,
        expected: usize,
        duration: Duration,
    ) -> Result<()> {
        let path = self.path(rel);
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            let current = if path.exists() {
                std::fs::read_to_string(&path)?.lines().count()
            } else {
                0
            };
            if current != expected {
                let body = std::fs::read_to_string(&path).unwrap_or_default();
                return Err(anyhow!(
                    "expected {} to stay at {expected} lines for {duration:?}, found {current}\ncontents:\n{}",
                    path.display(),
                    body,
                ));
            }
            sleep(POLL_INTERVAL).await;
        }
        Ok(())
    }
}
