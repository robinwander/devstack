use std::collections::BTreeMap;

/// Resolve `$VAR` and `${VAR}` environment variable references in a string.
/// Looks up variables from the current process environment (`std::env::vars()`).
pub fn resolve_env_vars(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '$' {
            // Check for ${VAR} syntax
            if chars.peek() == Some(&'{') {
                chars.next(); // consume '{'
                let mut var_name = String::new();
                let mut closed = false;
                // Collect until closing brace
                for c in chars.by_ref() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    var_name.push(c);
                }
                if let Ok(env_val) = std::env::var(&var_name) {
                    result.push_str(&env_val);
                } else {
                    // Variable not found, keep the original ${VAR} token.
                    result.push_str("${");
                    result.push_str(&var_name);
                    if closed {
                        result.push('}');
                    }
                }
            } else {
                // $VAR syntax - consume alphanumeric and underscore characters
                let mut var_name = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_alphanumeric() || c == '_' {
                        var_name.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if !var_name.is_empty() {
                    if let Ok(env_val) = std::env::var(&var_name) {
                        result.push_str(&env_val);
                    } else {
                        // Variable not found, keep the original $VAR
                        result.push('$');
                        result.push_str(&var_name);
                    }
                } else {
                    // Lone $ at end or followed by non-identifier char
                    result.push('$');
                }
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Resolve environment variables in all values of an env map.
pub fn resolve_env_map(env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    env.iter()
        .map(|(k, v)| (k.clone(), resolve_env_vars(v)))
        .collect()
}
