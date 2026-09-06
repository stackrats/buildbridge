//! Env sets: vault records, rendering into the guest, and the commands that keep them.

use super::*;
pub async fn list_env_sets(app: &Engine) -> Result<Vec<EnvSetSummary>, String> {
    let sets = read_env_sets().await?.sets;
    let registry = machines::load_registry(app)?;

    Ok(sets
        .iter()
        .map(|set| summarize_env_set(set, &registry.machines))
        .collect())
}
pub async fn save_env_set(app: &Engine, input: EnvSetInput) -> Result<Vec<EnvSetSummary>, String> {
    let name = input.name.trim().to_string();
    if name.is_empty() || name.len() > 60 {
        return Err("Give the environment a name of up to 60 characters.".to_string());
    }
    let mut sets = read_env_sets().await?;

    match input.set_id.clone() {
        Some(id) => {
            let index = sets
                .sets
                .iter()
                .position(|set| set.id == id)
                .ok_or_else(|| "This environment is no longer stored.".to_string())?;
            let variables = merge_env_variables(Some(&sets.sets[index]), &input)?;
            sets.sets[index].name = name;
            sets.sets[index].variables = variables;
        }
        None => {
            if sets.sets.len() >= MAX_ENV_SETS {
                return Err(format!(
                    "BuildBridge stores at most {MAX_ENV_SETS} environments."
                ));
            }
            let variables = merge_env_variables(None, &input)?;
            let existing_ids = sets
                .sets
                .iter()
                .map(|set| set.id.as_str())
                .collect::<Vec<_>>();
            sets.sets.push(StoredEnvSet {
                id: machines::machine_id_from_name(&name, &existing_ids),
                name,
                variables,
                created_at_epoch_seconds: machines::now_epoch_seconds(),
            });
        }
    }

    write_env_sets(sets).await?;

    list_env_sets(app).await
}
pub async fn delete_env_set(
    app: &Engine,
    set_id: String,
    input: ConfirmInput,
) -> Result<Vec<EnvSetSummary>, String> {
    if !input.confirmed {
        return Err("Confirm removing the environment before continuing.".to_string());
    }
    let mut sets = read_env_sets().await?;
    let index = sets
        .sets
        .iter()
        .position(|set| set.id == set_id)
        .ok_or_else(|| "This environment is no longer stored.".to_string())?;
    sets.sets.remove(index);
    write_env_sets(sets).await?;

    let mut registry = machines::load_registry(app)?;
    let mut detached = false;
    for machine in &mut registry.machines {
        if machine.env_set_id.as_deref() == Some(set_id.as_str()) {
            machine.env_set_id = None;
            detached = true;
        }
    }
    if detached {
        machines::save_registry(app, &registry)?;
    }

    list_env_sets(app).await
}
pub async fn attach_env_set(
    app: &Engine,
    machine_id: String,
    input: AttachEnvSetInput,
) -> Result<MachineView, String> {
    let paths = MachinePaths::resolve(app, &machine_id)?;
    if let Some(id) = input.set_id.as_deref() {
        let stored = read_env_sets().await?;
        if !stored.sets.iter().any(|set| set.id == id) {
            return Err("This environment is no longer stored.".to_string());
        }
    }
    let mut registry = machines::load_registry(app)?;
    let index = registry.position(&machine_id)?;
    registry.machines[index].env_set_id = input.set_id;
    machines::save_registry(app, &registry)?;

    build_machine_view(app, &paths).await
}

/// Every secret in one set with its value, for the editor to hold masked behind an eye icon. This
/// is the only way a secret leaves the vault for the interface: by set, on request, and never
/// inside a summary.
pub async fn reveal_env_secrets(set_id: String) -> Result<Vec<EnvVariableSummary>, String> {
    let stored = read_env_sets().await?;

    stored_env_secrets(&stored, &set_id)
}

pub(crate) fn env_set_credential_entry() -> Result<Entry, String> {
    Entry::new(ENV_SET_CREDENTIAL_SERVICE, SIGNING_KIT_CREDENTIAL_ACCOUNT)
        .map_err(|error| error.to_string())
}

pub(crate) async fn read_env_sets() -> Result<StoredEnvSets, String> {
    tokio::task::spawn_blocking(move || {
        let entry = env_set_credential_entry()?;

        match entry.get_password() {
            Ok(encoded) => serde_json::from_str::<StoredEnvSets>(&encoded)
                .map_err(|error| format!("The environment vault entry is invalid: {error}")),
            Err(keyring::Error::NoEntry) => Ok(StoredEnvSets::default()),
            Err(error) => Err(format!(
                "The operating-system credential vault could not be read: {error}"
            )),
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

pub(crate) async fn write_env_sets(sets: StoredEnvSets) -> Result<(), String> {
    let encoded = serde_json::to_string(&sets).map_err(|error| error.to_string())?;

    tokio::task::spawn_blocking(move || {
        env_set_credential_entry()?
            .set_password(&encoded)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

pub(crate) fn attached_env_set_id(
    app: &Engine,
    machine_id: &str,
) -> Result<Option<String>, String> {
    Ok(machines::load_registry(app)?
        .find(machine_id)
        .ok()
        .and_then(|machine| machine.env_set_id.clone()))
}

/// The attached set rendered for the guest, or `None` when the machine has none. A missing set
/// is an error rather than a silent build without variables, because that is how a production
/// build ends up pointed at the wrong backend.
pub(crate) async fn guest_env_files_for(
    app: &Engine,
    machine_id: &str,
) -> Result<Option<GuestEnvFiles>, String> {
    Ok(
        guest_env_files_for_set(attached_env_set_id(app, machine_id)?.as_deref())
            .await?
            .map(|(_, files)| files),
    )
}

/// One stored set rendered for the guest, with its name; `None` for no set at all.
pub(crate) async fn guest_env_files_for_set(
    set_id: Option<&str>,
) -> Result<Option<(String, GuestEnvFiles)>, String> {
    let Some(id) = set_id else {
        return Ok(None);
    };
    let stored = read_env_sets().await?;
    let set = stored
        .sets
        .into_iter()
        .find(|set| set.id == id)
        .ok_or_else(|| {
            "That environment is no longer stored. Choose another or build without one.".to_string()
        })?;

    Ok(Some((
        set.name.clone(),
        GuestEnvFiles {
            dotenv: render_dotenv(&set.variables),
            shell: render_shell_env(&set.variables),
        },
    )))
}

/// Plain values are already in every summary, so only the secrets come back this way.
pub(crate) fn stored_env_secrets(
    stored: &StoredEnvSets,
    set_id: &str,
) -> Result<Vec<EnvVariableSummary>, String> {
    let set = stored
        .sets
        .iter()
        .find(|set| set.id == set_id)
        .ok_or_else(|| "This environment is no longer stored.".to_string())?;

    Ok(set
        .variables
        .iter()
        .filter(|variable| variable.secret)
        .map(|variable| EnvVariableSummary {
            key: variable.key.clone(),
            value: variable.value.clone(),
        })
        .collect())
}

pub(crate) fn summarize_env_set(
    set: &StoredEnvSet,
    machines: &[machines::StoredMachine],
) -> EnvSetSummary {
    EnvSetSummary {
        id: set.id.clone(),
        name: set.name.clone(),
        variables: set
            .variables
            .iter()
            .filter(|variable| !variable.secret)
            .map(|variable| EnvVariableSummary {
                key: variable.key.clone(),
                value: variable.value.clone(),
            })
            .collect(),
        secret_keys: set
            .variables
            .iter()
            .filter(|variable| variable.secret)
            .map(|variable| variable.key.clone())
            .collect(),
        created_at_epoch_seconds: set.created_at_epoch_seconds,
        attached_machines: machines
            .iter()
            .filter(|machine| machine.env_set_id.as_deref() == Some(set.id.as_str()))
            .map(|machine| machine.config.name.clone())
            .collect(),
    }
}

/// A variable name as every dotenv loader and POSIX shell agree on it.
pub(crate) fn valid_env_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 120
        && key
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// Why a value cannot be stored, if it cannot. Both renderings below have to read it back the
/// same way, which rules out line breaks and a value that mixes both kinds of quote.
pub(crate) fn env_value_issue(value: &str) -> Option<&'static str> {
    if value.len() > MAX_ENV_VALUE_LENGTH {
        return Some("is longer than 4096 characters");
    }
    if value
        .chars()
        .any(|c| c == '\n' || c == '\r' || (c.is_control() && c != '\t'))
    {
        return Some("cannot contain line breaks or control characters");
    }
    if value.contains('"') && value.contains('\'') {
        return Some("cannot contain both single and double quotes");
    }

    None
}

/// Applies an edit to a set: every listed key with a typed value takes it, a listed key with no
/// value keeps what is stored, and an unlisted stored key is removed. A stored secret is never
/// carried over as a plain variable, since that would show a value stored on the promise that it
/// never would be.
pub(crate) fn merge_env_variables(
    existing: Option<&StoredEnvSet>,
    input: &EnvSetInput,
) -> Result<Vec<StoredEnvVariable>, String> {
    if input.variables.len() > MAX_ENV_VARIABLES {
        return Err(format!(
            "An environment holds at most {MAX_ENV_VARIABLES} variables."
        ));
    }
    let mut seen = std::collections::HashSet::new();
    let mut variables = Vec::with_capacity(input.variables.len());
    for variable in &input.variables {
        let key = variable.key.trim();
        if !valid_env_key(key) {
            return Err(format!(
                "\"{key}\" is not a valid variable name. Use letters, digits and underscores, not starting with a digit."
            ));
        }
        if !seen.insert(key.to_string()) {
            return Err(format!("{key} is listed twice."));
        }
        let value = match &variable.value {
            Some(value) => value.clone(),
            None => {
                let stored = existing
                    .and_then(|set| set.variables.iter().find(|stored| stored.key == key))
                    .ok_or_else(|| format!("{key} needs a value."))?;
                if stored.secret && !variable.secret {
                    return Err(format!(
                        "{key} is stored as a secret. Enter its value to keep it as a variable."
                    ));
                }
                stored.value.clone()
            }
        };
        if let Some(issue) = env_value_issue(&value) {
            return Err(format!("The value of {key} {issue}."));
        }
        variables.push(StoredEnvVariable {
            key: key.to_string(),
            value,
            secret: variable.secret,
        });
    }

    Ok(variables)
}

/// The set as Vite's dotenv loader reads it. Double quotes with `$` escaped so nothing expands;
/// single quotes when the value itself holds a double quote, which dotenv takes verbatim.
pub(crate) fn render_dotenv(variables: &[StoredEnvVariable]) -> String {
    let mut out = String::from(
        "# Written by BuildBridge from the attached environment. Not part of the project.\n",
    );
    for variable in variables {
        out.push_str(&variable.key);
        out.push('=');
        if variable.value.contains('"') {
            out.push('\'');
            out.push_str(&variable.value);
            out.push('\'');
        } else {
            out.push('"');
            for character in variable.value.chars() {
                match character {
                    '\\' => out.push_str("\\\\"),
                    '$' => out.push_str("\\$"),
                    '`' => out.push_str("\\`"),
                    other => out.push(other),
                }
            }
            out.push('"');
        }
        out.push('\n');
    }

    out
}

/// The same set as a POSIX shell sources it: single-quoted, which is exact for anything but a
/// single quote, and that is spelled `'\''`.
pub(crate) fn render_shell_env(variables: &[StoredEnvVariable]) -> String {
    let mut out = String::from("# Written by BuildBridge from the attached environment.\n");
    for variable in variables {
        out.push_str("export ");
        out.push_str(&variable.key);
        out.push_str("='");
        out.push_str(&variable.value.replace('\'', "'\\''"));
        out.push_str("'\n");
    }

    out
}
