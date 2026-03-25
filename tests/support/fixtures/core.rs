use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use toml::Value;

pub trait FixtureSpec {
    fn name(&self) -> &'static str;
    fn render(&self) -> Result<RenderedFixture>;
}

#[derive(Clone, Default)]
pub struct RenderedFixture {
    pub files: BTreeMap<PathBuf, Vec<u8>>,
}

impl RenderedFixture {
    pub fn text(mut self, path: impl AsRef<Path>, contents: impl AsRef<str>) -> Self {
        self.files.insert(
            path.as_ref().to_path_buf(),
            contents.as_ref().as_bytes().to_vec(),
        );
        self
    }

    pub fn bytes(mut self, path: impl AsRef<Path>, contents: impl Into<Vec<u8>>) -> Self {
        self.files
            .insert(path.as_ref().to_path_buf(), contents.into());
        self
    }

    pub fn merge(&mut self, other: RenderedFixture) {
        self.files.extend(other.files);
    }
}

pub struct FixtureConfig {
    value: Value,
}

impl FixtureConfig {
    pub fn parse_toml(input: &str) -> Result<Self> {
        let value = input.parse::<Value>().context("parse fixture toml")?;
        Ok(Self { value })
    }

    pub fn to_toml_string(&self) -> Result<String> {
        toml::to_string_pretty(&self.value).context("serialize fixture toml")
    }

    pub fn service(&mut self, stack: &str, service: &str) -> Result<ServiceConfigPatch<'_>> {
        let table = table_at_path_mut(
            &mut self.value,
            &["stacks", stack, "services", service],
            false,
        )?;
        Ok(ServiceConfigPatch { table })
    }

    pub fn global_service(&mut self, service: &str) -> Result<ServiceConfigPatch<'_>> {
        let table = table_at_path_mut(&mut self.value, &["globals", service], false)?;
        Ok(ServiceConfigPatch { table })
    }

    pub fn remove_service(&mut self, stack: &str, service: &str) -> Result<()> {
        let services = table_at_path_mut(&mut self.value, &["stacks", stack, "services"], false)?;
        services.remove(service);
        Ok(())
    }

    pub fn task(&mut self, task: &str) -> Result<TaskConfigPatch<'_>> {
        let table = table_at_path_mut(&mut self.value, &["tasks", task], true)?;
        Ok(TaskConfigPatch { table })
    }
}

pub struct ServiceConfigPatch<'a> {
    table: &'a mut toml::map::Map<String, Value>,
}

impl ServiceConfigPatch<'_> {
    pub fn cmd(&mut self, value: impl Into<String>) -> &mut Self {
        self.table
            .insert("cmd".to_string(), Value::String(value.into()));
        self
    }

    pub fn auto_restart(&mut self, enabled: bool) -> &mut Self {
        self.table
            .insert("auto_restart".to_string(), Value::Boolean(enabled));
        self
    }

    pub fn port_none(&mut self) -> &mut Self {
        self.table
            .insert("port".to_string(), Value::String("none".to_string()));
        self
    }

    pub fn port_fixed(&mut self, port: u16) -> &mut Self {
        self.table
            .insert("port".to_string(), Value::Integer(port as i64));
        self
    }

    pub fn watch(&mut self, patterns: &[&str]) -> &mut Self {
        self.table.insert(
            "watch".to_string(),
            Value::Array(
                patterns
                    .iter()
                    .map(|value| Value::String((*value).to_string()))
                    .collect(),
            ),
        );
        self
    }

    pub fn ignore(&mut self, patterns: &[&str]) -> &mut Self {
        self.table.insert(
            "ignore".to_string(),
            Value::Array(
                patterns
                    .iter()
                    .map(|value| Value::String((*value).to_string()))
                    .collect(),
            ),
        );
        self
    }

    pub fn init(&mut self, task_names: &[&str]) -> &mut Self {
        self.table.insert(
            "init".to_string(),
            Value::Array(
                task_names
                    .iter()
                    .map(|value| Value::String((*value).to_string()))
                    .collect(),
            ),
        );
        self
    }

    pub fn post_init(&mut self, task_names: &[&str]) -> &mut Self {
        self.table.insert(
            "post_init".to_string(),
            Value::Array(
                task_names
                    .iter()
                    .map(|value| Value::String((*value).to_string()))
                    .collect(),
            ),
        );
        self
    }

    pub fn clear_readiness(&mut self) -> &mut Self {
        self.table.remove("readiness");
        self
    }

    pub fn readiness_tcp(&mut self) -> &mut Self {
        self.table.insert(
            "readiness".to_string(),
            readiness_table([("tcp", Value::Table(toml::map::Map::new()))]),
        );
        self
    }

    pub fn readiness_http(
        &mut self,
        path: impl Into<String>,
        expect_status: [u16; 2],
    ) -> &mut Self {
        let mut http = toml::map::Map::new();
        http.insert("path".to_string(), Value::String(path.into()));
        http.insert(
            "expect_status".to_string(),
            Value::Array(
                expect_status
                    .iter()
                    .map(|value| Value::Integer(i64::from(*value)))
                    .collect(),
            ),
        );
        self.table.insert(
            "readiness".to_string(),
            readiness_table([("http", Value::Table(http))]),
        );
        self
    }

    pub fn readiness_delay_ms(&mut self, delay_ms: u64) -> &mut Self {
        self.table.insert(
            "readiness".to_string(),
            readiness_table([("delay_ms", Value::Integer(delay_ms as i64))]),
        );
        self
    }

    pub fn readiness_log_regex(&mut self, pattern: impl Into<String>) -> &mut Self {
        self.table.insert(
            "readiness".to_string(),
            readiness_table([("log_regex", Value::String(pattern.into()))]),
        );
        self
    }

    pub fn readiness_cmd(&mut self, command: impl Into<String>) -> &mut Self {
        self.table.insert(
            "readiness".to_string(),
            readiness_table([("cmd", Value::String(command.into()))]),
        );
        self
    }

    pub fn readiness_exit(&mut self) -> &mut Self {
        self.table.insert(
            "readiness".to_string(),
            readiness_table([("exit", Value::Table(toml::map::Map::new()))]),
        );
        self
    }

    pub fn readiness_timeout_ms(&mut self, timeout_ms: u64) -> Result<&mut Self> {
        let readiness = nested_table_mut(self.table, "readiness")?;
        readiness.insert("timeout_ms".to_string(), Value::Integer(timeout_ms as i64));
        Ok(self)
    }

    pub fn env(&mut self, key: impl Into<String>, value: impl Into<String>) -> Result<&mut Self> {
        let env = nested_table_mut(self.table, "env")?;
        env.insert(key.into(), Value::String(value.into()));
        Ok(self)
    }
}

pub struct TaskConfigPatch<'a> {
    table: &'a mut toml::map::Map<String, Value>,
}

impl TaskConfigPatch<'_> {
    pub fn cmd(&mut self, value: impl Into<String>) -> &mut Self {
        self.table
            .insert("cmd".to_string(), Value::String(value.into()));
        self
    }

    pub fn watch(&mut self, patterns: &[&str]) -> &mut Self {
        self.table.insert(
            "watch".to_string(),
            Value::Array(
                patterns
                    .iter()
                    .map(|value| Value::String((*value).to_string()))
                    .collect(),
            ),
        );
        self
    }

    pub fn env(&mut self, key: impl Into<String>, value: impl Into<String>) -> Result<&mut Self> {
        let env = nested_table_mut(self.table, "env")?;
        env.insert(key.into(), Value::String(value.into()));
        Ok(self)
    }
}

pub(crate) fn base_http_fixture_toml(auto_restart: bool, post_init: bool) -> String {
    let auto_restart_lines = if auto_restart {
        "auto_restart = true\nwatch = [\"src/**\"]\nignore = [\"ignored/**\"]\n"
    } else {
        ""
    };
    let post_init_lines = if post_init {
        "post_init = [\"post-init\"]\n"
    } else {
        ""
    };
    let tasks_block = if post_init {
        r#"
[tasks.post-init]
cmd = "bin/append-marker.sh state/post-init.log post-init"
"#
    } else {
        ""
    };

    format!(
        r#"version = 1
default_stack = "dev"

[stacks.dev.services.api]
cmd = "python3 bin/service_http.py"
{auto_restart_lines}{post_init_lines}

[stacks.dev.services.api.env]
FIXTURE_SERVICE_NAME = "api"
FIXTURE_STARTS_FILE = "state/api-starts.log"

[stacks.dev.services.api.readiness.http]
path = "/"
expect_status = [200, 299]
{tasks_block}
"#
    )
}

fn readiness_table(items: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    let mut readiness = toml::map::Map::new();
    for (key, value) in items {
        readiness.insert(key.to_string(), value);
    }
    Value::Table(readiness)
}

fn table_at_path_mut<'a>(
    value: &'a mut Value,
    path: &[&str],
    create_missing: bool,
) -> Result<&'a mut toml::map::Map<String, Value>> {
    let mut current = value
        .as_table_mut()
        .ok_or_else(|| anyhow!("fixture config root is not a TOML table"))?;

    for segment in path {
        if create_missing {
            let entry = current
                .entry((*segment).to_string())
                .or_insert_with(|| Value::Table(toml::map::Map::new()));
            current = entry
                .as_table_mut()
                .ok_or_else(|| anyhow!("fixture path segment {segment:?} is not a table"))?;
        } else {
            let next = current
                .get_mut(*segment)
                .ok_or_else(|| anyhow!("fixture path segment {segment:?} is missing"))?;
            current = next
                .as_table_mut()
                .ok_or_else(|| anyhow!("fixture path segment {segment:?} is not a table"))?;
        }
    }

    Ok(current)
}

fn nested_table_mut<'a>(
    table: &'a mut toml::map::Map<String, Value>,
    key: &str,
) -> Result<&'a mut toml::map::Map<String, Value>> {
    let entry = table
        .entry(key.to_string())
        .or_insert_with(|| Value::Table(toml::map::Map::new()));
    entry
        .as_table_mut()
        .ok_or_else(|| anyhow!("fixture key {key:?} is not a table"))
}
