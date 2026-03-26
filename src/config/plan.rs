use std::collections::{BTreeMap, VecDeque};

use anyhow::{Result, anyhow};

use super::model::{ConfigFile, ServiceConfig, StackPlan};

impl ConfigFile {
    pub fn stack_plan(&self, name: &str) -> Result<StackPlan> {
        let stack = self
            .stacks
            .as_map()
            .get(name)
            .ok_or_else(|| anyhow!("stack {name} not found"))?;
        let services = stack.services.as_map().clone();
        let order = topo_sort(&services)?;
        Ok(StackPlan {
            name: name.to_string(),
            services,
            order,
        })
    }

    pub fn globals_map(&self) -> BTreeMap<String, ServiceConfig> {
        self.globals
            .as_ref()
            .map(|map| map.as_map().clone())
            .unwrap_or_default()
    }
}

pub fn topo_sort(services: &BTreeMap<String, ServiceConfig>) -> Result<Vec<String>> {
    let mut in_degree = BTreeMap::new();
    let mut graph = BTreeMap::new();

    // Initialize in_degree and graph
    for service_name in services.keys() {
        in_degree.insert(service_name.clone(), 0);
        graph.insert(service_name.clone(), Vec::new());
    }

    // Build the dependency graph and count in_degrees
    for (service_name, service) in services {
        for dep in &service.deps {
            // We already validate that deps reference existing services.
            if let Some(dep_edges) = graph.get_mut(dep) {
                dep_edges.push(service_name.clone());
                *in_degree.get_mut(service_name).unwrap() += 1;
            }
        }
    }

    // Kahn's algorithm for topological sort
    let mut queue = VecDeque::new();
    for (service, &degree) in &in_degree {
        if degree == 0 {
            queue.push_back(service.clone());
        }
    }

    let mut result = Vec::new();
    while let Some(service) = queue.pop_front() {
        result.push(service.clone());

        if let Some(dependents) = graph.get(&service) {
            for dependent in dependents {
                let new_degree = in_degree.get_mut(dependent).unwrap();
                *new_degree -= 1;
                if *new_degree == 0 {
                    queue.push_back(dependent.clone());
                }
            }
        }
    }

    if result.len() != services.len() {
        return Err(anyhow!("circular dependency detected"));
    }

    Ok(result)
}