use anyhow::{Result, anyhow};

use crate::util::validate_name_for_path_component;
use super::model::{ConfigFile, ServiceConfig, TaskConfig, UniqueMap, PortConfig};
use super::plan::topo_sort;

impl ConfigFile {
    pub(crate) fn validate(&self) -> Result<()> {
        if let Some(default_stack) = &self.default_stack
            && !self.stacks.as_map().contains_key(default_stack)
        {
            return Err(anyhow!(
                "default_stack '{default_stack}' not found in stacks"
            ));
        }
        for (stack_name, stack) in self.stacks.as_map() {
            let services = stack.services.as_map();
            for (svc_name, svc) in services {
                validate_name_for_path_component("service", svc_name).map_err(|err| {
                    anyhow!("invalid service name in stack '{stack_name}': {err}")
                })?;
                validate_service_port(stack_name, svc_name, svc)?;
                validate_service_readiness(stack_name, svc_name, svc)?;
                validate_service_init_tasks(stack_name, svc_name, svc, self.tasks.as_ref())?;
                validate_service_post_init_tasks(stack_name, svc_name, svc, self.tasks.as_ref())?;
                validate_service_auto_restart(stack_name, svc_name, svc)?;
                // Validate deps reference existing services in this stack.
                for dep in &svc.deps {
                    if !services.contains_key(dep) {
                        return Err(anyhow!(
                            "service '{svc_name}' in stack '{stack_name}' depends on \
                             unknown service '{dep}'"
                        ));
                    }
                }
            }
            // Validate no circular dependencies via topological sort.
            topo_sort(services).map_err(|err| anyhow!("stack '{stack_name}': {err}"))?;
        }
        if let Some(tasks) = &self.tasks {
            for task_name in tasks.as_map().keys() {
                validate_name_for_path_component("task", task_name)
                    .map_err(|err| anyhow!("invalid task name: {err}"))?;
            }
        }
        if let Some(globals) = &self.globals {
            for (svc_name, svc) in globals.as_map() {
                validate_name_for_path_component("service", svc_name)
                    .map_err(|err| anyhow!("invalid global service name: {err}"))?;
                validate_service_port("globals", svc_name, svc)?;
                validate_service_readiness("globals", svc_name, svc)?;
                validate_service_init_tasks("globals", svc_name, svc, self.tasks.as_ref())?;
                validate_service_post_init_tasks("globals", svc_name, svc, self.tasks.as_ref())?;
                validate_service_auto_restart("globals", svc_name, svc)?;
                if !svc.deps.is_empty() {
                    return Err(anyhow!("global service '{svc_name}' cannot have deps"));
                }
            }
        }
        Ok(())
    }
}

fn validate_service_port(stack: &str, service: &str, svc: &ServiceConfig) -> Result<()> {
    if let Some(port) = &svc.port {
        match port {
            PortConfig::Fixed(port_num) if !(1024..=65535).contains(port_num) => {
                return Err(anyhow!("service {service} in stack {stack}: port must be in range 1024-65535, got {port_num}"));
            }
            PortConfig::None(value) if value != "none" => {
                return Err(anyhow!("service {service} in stack {stack}: invalid port value \"{value}\", must be a number or \"none\""));
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_service_readiness(stack: &str, service: &str, svc: &ServiceConfig) -> Result<()> {
    let has_port = svc.port.as_ref().map(|p| !p.is_none()).unwrap_or(true);
    svc.readiness_kind(has_port).map_err(|err| {
        anyhow!("service {service} in stack {stack}: readiness error: {err}")
    })?;
    Ok(())
}

fn validate_service_init_tasks(
    stack: &str,
    service: &str,
    svc: &ServiceConfig,
    tasks: Option<&UniqueMap<String, TaskConfig>>,
) -> Result<()> {
    if let Some(init_tasks) = &svc.init {
        if tasks.is_none() {
            return Err(anyhow!(
                "service {service} in stack {stack}: references init tasks but no [tasks] are defined"
            ));
        }
        let available_tasks = tasks.unwrap().as_map();
        for init_task in init_tasks {
            if !available_tasks.contains_key(init_task) {
                return Err(anyhow!(
                    "service {service} in stack {stack}: unknown init task '{init_task}'"
                ));
            }
        }
    }
    Ok(())
}

fn validate_service_post_init_tasks(
    stack: &str,
    service: &str,
    svc: &ServiceConfig,
    tasks: Option<&UniqueMap<String, TaskConfig>>,
) -> Result<()> {
    if let Some(post_init_tasks) = &svc.post_init {
        if tasks.is_none() {
            return Err(anyhow!(
                "service {service} in stack {stack}: references post_init tasks but no [tasks] are defined"
            ));
        }
        let available_tasks = tasks.unwrap().as_map();
        for post_init_task in post_init_tasks {
            if !available_tasks.contains_key(post_init_task) {
                return Err(anyhow!(
                    "service {service} in stack {stack}: unknown post_init task '{post_init_task}'"
                ));
            }
        }
    }
    Ok(())
}

fn validate_service_auto_restart(stack: &str, service: &str, svc: &ServiceConfig) -> Result<()> {
    if svc.auto_restart && svc.watch.is_empty() {
        return Err(anyhow!(
            "service {service} in stack {stack}: auto_restart requires watch patterns"
        ));
    }
    Ok(())
}