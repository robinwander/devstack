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

    pub fn stack_plan_filtered(&self, name: &str, only: &[String]) -> Result<StackPlan> {
        let plan = self.stack_plan(name)?;
        if only.is_empty() {
            return Ok(plan);
        }
        plan.filter_to(only)
    }

    pub fn globals_map(&self) -> BTreeMap<String, ServiceConfig> {
        self.globals
            .as_ref()
            .map(|map| map.as_map().clone())
            .unwrap_or_default()
    }
}

impl StackPlan {
    pub fn filter_to(&self, targets: &[String]) -> Result<Self> {
        let mut needed = std::collections::BTreeSet::new();
        let mut queue: VecDeque<String> = targets.iter().cloned().collect();
        while let Some(name) = queue.pop_front() {
            if !self.services.contains_key(&name) {
                return Err(anyhow!("unknown service '{name}' in stack '{}'", self.name));
            }
            if needed.insert(name.clone()) {
                for dep in &self.services[&name].deps {
                    queue.push_back(dep.clone());
                }
            }
        }
        let services: BTreeMap<String, ServiceConfig> = self
            .services
            .iter()
            .filter(|(name, _)| needed.contains(*name))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let order: Vec<String> = self
            .order
            .iter()
            .filter(|name| needed.contains(*name))
            .cloned()
            .collect();
        Ok(StackPlan {
            name: self.name.clone(),
            services,
            order,
        })
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
