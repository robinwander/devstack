#[cfg(test)]
mod tests {
    use crate::model::ReadinessKind;
    use std::collections::BTreeMap;
    use std::fs;

    // Import the config types and functions
    use crate::config::env;
    use crate::config::model::*;
    use crate::config::plan;

    #[test]
    fn topo_sort_orders_deps() {
        let mut services = BTreeMap::new();
        services.insert(
            "a".to_string(),
            ServiceConfig {
                cmd: "echo a".to_string(),
                deps: vec![],
                scheme: None,
                port_env: None,
                port: None,
                readiness: None,
                env_file: None,
                env: BTreeMap::new(),
                cwd: None,
                watch: Vec::new(),
                ignore: Vec::new(),
                auto_restart: false,
                init: None,
                post_init: None,
            },
        );
        services.insert(
            "b".to_string(),
            ServiceConfig {
                cmd: "echo b".to_string(),
                deps: vec!["a".to_string()],
                scheme: None,
                port_env: None,
                port: None,
                readiness: None,
                env_file: None,
                env: BTreeMap::new(),
                cwd: None,
                watch: Vec::new(),
                ignore: Vec::new(),
                auto_restart: false,
                init: None,
                post_init: None,
            },
        );
        let order = plan::topo_sort(&services).unwrap();
        assert_eq!(order, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn readiness_defaults_tcp_when_ported() {
        let svc = ServiceConfig {
            cmd: "echo".to_string(),
            deps: vec![],
            scheme: None,
            port_env: None,
            port: None,
            readiness: None,
            env_file: None,
            env: BTreeMap::new(),
            cwd: None,
            watch: Vec::new(),
            ignore: Vec::new(),
            auto_restart: false,
            init: None,
            post_init: None,
        };
        let kind = svc.readiness_kind(true).unwrap();
        assert!(matches!(kind, ReadinessKind::Tcp));
    }

    #[test]
    fn readiness_none_when_no_port() {
        let svc = ServiceConfig {
            cmd: "echo".to_string(),
            deps: vec![],
            scheme: None,
            port_env: None,
            port: None,
            readiness: None,
            env_file: None,
            env: BTreeMap::new(),
            cwd: None,
            watch: Vec::new(),
            ignore: Vec::new(),
            auto_restart: false,
            init: None,
            post_init: None,
        };
        let kind = svc.readiness_kind(false).unwrap();
        assert!(matches!(kind, ReadinessKind::None));
    }

    #[test]
    fn readiness_http_range_parsed() {
        let svc = ServiceConfig {
            cmd: "echo".to_string(),
            deps: vec![],
            scheme: None,
            port_env: None,
            port: None,
            readiness: Some(ReadinessConfig {
                tcp: None,
                http: Some(ReadinessHttp {
                    path: "/health".to_string(),
                    expect_status: Some(vec![200, 204]),
                }),
                log_regex: None,
                cmd: None,
                delay_ms: None,
                exit: None,
                timeout_ms: None,
            }),
            env_file: None,
            env: BTreeMap::new(),
            cwd: None,
            watch: Vec::new(),
            ignore: Vec::new(),
            auto_restart: false,
            init: None,
            post_init: None,
        };
        let kind = svc.readiness_kind(true).unwrap();
        match kind {
            ReadinessKind::Http {
                path,
                expect_min,
                expect_max,
            } => {
                assert_eq!(path, "/health");
                assert_eq!(expect_min, 200);
                assert_eq!(expect_max, 204);
            }
            _ => panic!("expected http readiness"),
        }
    }

    #[test]
    fn readiness_delay_ms_selected() {
        let svc = ServiceConfig {
            cmd: "echo".to_string(),
            deps: vec![],
            scheme: None,
            port_env: None,
            port: None,
            readiness: Some(ReadinessConfig {
                tcp: None,
                http: None,
                log_regex: None,
                cmd: None,
                delay_ms: Some(1500),
                exit: None,
                timeout_ms: None,
            }),
            env_file: None,
            env: BTreeMap::new(),
            cwd: None,
            watch: Vec::new(),
            ignore: Vec::new(),
            auto_restart: false,
            init: None,
            post_init: None,
        };
        let kind = svc.readiness_kind(false).unwrap();
        match kind {
            ReadinessKind::Delay { duration } => {
                assert_eq!(duration, std::time::Duration::from_millis(1500));
            }
            _ => panic!("expected delay readiness"),
        }
    }

    #[test]
    fn readiness_exit_selected() {
        let svc = ServiceConfig {
            cmd: "echo".to_string(),
            deps: vec![],
            scheme: None,
            port_env: None,
            port: None,
            readiness: Some(ReadinessConfig {
                tcp: None,
                http: None,
                log_regex: None,
                cmd: None,
                delay_ms: None,
                exit: Some(ReadinessExit {}),
                timeout_ms: None,
            }),
            env_file: None,
            env: BTreeMap::new(),
            cwd: None,
            watch: Vec::new(),
            ignore: Vec::new(),
            auto_restart: false,
            init: None,
            post_init: None,
        };
        let kind = svc.readiness_kind(false).unwrap();
        assert!(matches!(kind, ReadinessKind::Exit));
    }

    #[test]
    fn duplicate_service_keys_error() {
        let yaml = r#"
version: 1
stacks:
  test:
    services:
      api:
        cmd: "echo api"
      api:
        cmd: "echo api2"
"#;
        let result: Result<ConfigFile, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn port_string_must_be_none() {
        let yaml = r#"
version: 1
stacks:
  test:
    services:
      api:
        cmd: "echo api"
        port: "off"
"#;
        let config: ConfigFile = serde_yaml::from_str(yaml).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("invalid port value"));
    }

    #[test]
    fn parses_toml() {
        let toml_str = r#"
version = 1

[stacks.app.services.api]
cmd = "echo api"
readiness = { tcp = {} }
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(config.version, 1);
        let plan = config.stack_plan("app").unwrap();
        assert!(plan.services.contains_key("api"));
    }

    #[test]
    fn parses_toml_delay_ms_readiness() {
        let toml_str = r#"
version = 1

[stacks.app.services.worker]
cmd = "echo worker"
port = "none"
readiness = { delay_ms = 5000 }
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let plan = config.stack_plan("app").unwrap();
        let svc = plan.services.get("worker").unwrap();
        let kind = svc.readiness_kind(false).unwrap();
        assert!(matches!(kind, ReadinessKind::Delay { .. }));
    }

    #[test]
    fn parses_auto_restart_field() {
        let toml_str = r#"
version = 1

[stacks.app.services.worker]
cmd = "echo worker"
watch = ["src/**"]
auto_restart = true

[stacks.app.services.web]
cmd = "echo web"
watch = ["src/**"]
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let plan = config.stack_plan("app").unwrap();
        assert!(plan.services.get("worker").unwrap().auto_restart);
        assert!(!plan.services.get("web").unwrap().auto_restart);
    }

    #[test]
    fn auto_restart_requires_watch_patterns() {
        let toml_str = r#"
version = 1

[stacks.app.services.worker]
cmd = "echo worker"
auto_restart = true
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(
            err.to_string()
                .contains("auto_restart requires watch patterns")
        );
    }

    #[test]
    fn default_stack_must_exist() {
        let toml_str = r#"
version = 1
default_stack = "missing"

[stacks.app.services.api]
cmd = "echo api"
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("default_stack"));
    }

    #[test]
    fn invalid_service_name_is_rejected() {
        let toml_str = r#"
version = 1

[stacks.app.services."api/../../escape"]
cmd = "echo api"
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("invalid service name"));
    }

    #[test]
    fn invalid_global_service_name_is_rejected() {
        let toml_str = r#"
version = 1

[stacks.app.services.api]
cmd = "echo api"

[globals."bad/../../global"]
cmd = "echo global"
readiness = { delay_ms = 1 }
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("invalid global service name"));
    }

    #[test]
    fn invalid_task_name_is_rejected() {
        let toml_str = r#"
version = 1

[stacks.app.services.api]
cmd = "echo api"

[tasks."task/../../escape"]
cmd = "echo task"
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("invalid task name"));
    }

    #[test]
    fn find_nearest_config_walks_upwards() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let nested = root.join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        let config = root.join("devstack.toml");
        fs::write(
            &config,
            "version = 1\n[stacks.app.services.api]\ncmd = \"echo\"",
        )
        .unwrap();
        let found = ConfigFile::find_nearest_path(&nested).unwrap();
        assert_eq!(found, config);
    }

    #[test]
    fn find_nearest_config_prefers_toml_over_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let yaml = root.join("devstack.yml");
        let toml = root.join("devstack.toml");
        fs::write(&yaml, "version: 1\nstacks: {}").unwrap();
        fs::write(
            &toml,
            "version = 1\n[stacks.app.services.api]\ncmd = \"echo\"",
        )
        .unwrap();
        let found = ConfigFile::find_nearest_path(root).unwrap();
        assert_eq!(found, toml);
    }

    #[test]
    fn resolve_env_vars_dollar_brace_syntax() {
        // Set a known env var for testing
        unsafe { std::env::set_var("DEVSTACK_TEST_VAR", "test_value") };
        let result = env::resolve_env_vars("value is ${DEVSTACK_TEST_VAR}");
        assert_eq!(result, "value is test_value");
    }

    #[test]
    fn resolve_env_vars_dollar_syntax() {
        unsafe { std::env::set_var("DEVSTACK_TEST_VAR2", "hello") };
        let result = env::resolve_env_vars("$DEVSTACK_TEST_VAR2 world");
        assert_eq!(result, "hello world");
    }

    #[test]
    fn resolve_env_vars_missing_var_keeps_placeholder() {
        let result = env::resolve_env_vars("$NONEXISTENT_VAR");
        assert_eq!(result, "$NONEXISTENT_VAR");
    }

    #[test]
    fn resolve_env_vars_missing_braced_var_keeps_placeholder() {
        let result = env::resolve_env_vars("${NONEXISTENT_BRACED}");
        assert_eq!(result, "${NONEXISTENT_BRACED}");
    }

    #[test]
    fn resolve_env_vars_no_interpolation() {
        let result = env::resolve_env_vars("plain value");
        assert_eq!(result, "plain value");
    }

    #[test]
    fn resolve_env_vars_mixed_content() {
        unsafe { std::env::set_var("DEVSTACK_MIXED", "mixed") };
        let result = env::resolve_env_vars("before ${DEVSTACK_MIXED} after");
        assert_eq!(result, "before mixed after");
    }

    #[test]
    fn resolve_env_vars_multiple_vars() {
        unsafe {
            std::env::set_var("VAR_A", "alpha");
            std::env::set_var("VAR_B", "beta");
        }
        let result = env::resolve_env_vars("$VAR_A and $VAR_B");
        assert_eq!(result, "alpha and beta");
    }

    #[test]
    fn resolve_env_map_resolves_all_values() {
        unsafe {
            std::env::set_var("DB_HOST", "localhost");
            std::env::set_var("DB_PORT", "5432");
        }
        let mut env_map = BTreeMap::new();
        env_map.insert("HOST".to_string(), "$DB_HOST".to_string());
        env_map.insert("PORT".to_string(), "${DB_PORT}".to_string());
        let resolved = env::resolve_env_map(&env_map);
        assert_eq!(resolved.get("HOST"), Some(&"localhost".to_string()));
        assert_eq!(resolved.get("PORT"), Some(&"5432".to_string()));
    }

    #[test]
    fn post_init_references_unknown_task() {
        let toml_str = r#"
version = 1

[tasks.setup]
cmd = "echo setup"

[stacks.app.services.api]
cmd = "echo api"
post_init = ["missing-task"]
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("unknown post_init task"));
    }

    #[test]
    fn post_init_without_tasks_section() {
        let toml_str = r#"
version = 1

[stacks.app.services.api]
cmd = "echo api"
post_init = ["setup"]
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("post_init tasks but no [tasks]"));
    }

    #[test]
    fn post_init_with_valid_task() {
        let toml_str = r#"
version = 1

[tasks.create-resources]
cmd = "python scripts/init.py"
watch = ["scripts/init.py"]

[stacks.app.services.api]
cmd = "echo api"
readiness = { http = { path = "/health" } }
post_init = ["create-resources"]
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        config.validate().unwrap();
        let plan = config.stack_plan("app").unwrap();
        let svc = plan.services.get("api").unwrap();
        assert_eq!(
            svc.post_init.as_deref(),
            Some(vec!["create-resources".to_string()].as_slice())
        );
    }

    #[test]
    fn global_post_init_references_known_task() {
        let toml_str = r#"
version = 1

[tasks.seed]
cmd = "echo seed"

[stacks.app.services.api]
cmd = "echo api"

[globals.moto]
cmd = "echo moto"
port = "none"
readiness = { delay_ms = 1 }
post_init = ["seed"]
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        config.validate().unwrap();
    }
}
