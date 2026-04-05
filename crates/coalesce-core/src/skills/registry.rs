use super::types::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

/// In-memory skill registry backed by JSON files on disk.
pub struct SkillRegistry {
    skills: RwLock<HashMap<String, Skill>>,
    storage_dir: PathBuf,
}

impl SkillRegistry {
    /// Create a new registry, loading any existing skills from disk.
    pub fn new() -> Self {
        let storage_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("coalesce")
            .join("skills");
        std::fs::create_dir_all(&storage_dir).ok();
        let registry = Self {
            skills: RwLock::new(HashMap::new()),
            storage_dir,
        };
        registry.load_from_disk();
        registry
    }

    /// Create a registry with a custom storage directory (useful for tests).
    pub fn with_dir(storage_dir: PathBuf) -> Self {
        std::fs::create_dir_all(&storage_dir).ok();
        let registry = Self {
            skills: RwLock::new(HashMap::new()),
            storage_dir,
        };
        registry.load_from_disk();
        registry
    }

    /// Load all skill JSON files from the storage directory.
    fn load_from_disk(&self) {
        let Ok(entries) = std::fs::read_dir(&self.storage_dir) else {
            return;
        };
        let mut map = self.skills.write().unwrap();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Ok(contents) = std::fs::read_to_string(&path) {
                    if let Ok(skill) = serde_json::from_str::<Skill>(&contents) {
                        map.insert(skill.id.clone(), skill);
                    }
                }
            }
        }
    }

    /// Persist a single skill to disk.
    fn save_to_disk(&self, skill: &Skill) -> std::io::Result<()> {
        let path = self.storage_dir.join(format!("{}.json", skill.id));
        let json = serde_json::to_string_pretty(skill)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(path, json)
    }

    /// Install a skill. Overwrites if same id exists.
    pub fn install(&self, skill: Skill) -> Result<(), String> {
        self.save_to_disk(&skill).map_err(|e| e.to_string())?;
        self.skills.write().unwrap().insert(skill.id.clone(), skill);
        Ok(())
    }

    /// Uninstall a skill, moving its file to a backup directory.
    pub fn uninstall(&self, id: &str) -> Result<Skill, String> {
        let mut map = self.skills.write().unwrap();
        let skill = map.remove(id).ok_or_else(|| format!("skill '{}' not found", id))?;

        // Move file to backup dir
        let backup_dir = self.storage_dir.join(".backup");
        std::fs::create_dir_all(&backup_dir).ok();
        let src = self.storage_dir.join(format!("{}.json", id));
        let dst = backup_dir.join(format!("{}.json", id));
        if src.exists() {
            std::fs::rename(&src, &dst).ok();
        }
        Ok(skill)
    }

    /// List all installed skills.
    pub fn list(&self) -> Vec<Skill> {
        self.skills.read().unwrap().values().cloned().collect()
    }

    /// Get a single skill by id.
    pub fn get(&self, id: &str) -> Option<Skill> {
        self.skills.read().unwrap().get(id).cloned()
    }

    /// Toggle a skill's enabled state. Returns the new state.
    pub fn toggle(&self, id: &str) -> Result<bool, String> {
        let mut map = self.skills.write().unwrap();
        let skill = map.get_mut(id).ok_or_else(|| format!("skill '{}' not found", id))?;
        skill.enabled = !skill.enabled;
        let new_state = skill.enabled;
        let skill_clone = skill.clone();
        drop(map);
        self.save_to_disk(&skill_clone).map_err(|e| e.to_string())?;
        Ok(new_state)
    }

    /// Toggle whether a harness is associated with a skill.
    pub fn toggle_harness(&self, id: &str, harness: &str) -> Result<(), String> {
        let mut map = self.skills.write().unwrap();
        let skill = map.get_mut(id).ok_or_else(|| format!("skill '{}' not found", id))?;
        if let Some(pos) = skill.harnesses.iter().position(|h| h == harness) {
            skill.harnesses.remove(pos);
        } else {
            skill.harnesses.push(harness.to_string());
        }
        let skill_clone = skill.clone();
        drop(map);
        self.save_to_disk(&skill_clone).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Number of installed skills.
    pub fn count(&self) -> usize {
        self.skills.read().unwrap().len()
    }

    /// Get all skills enabled for a specific harness.
    pub fn for_harness(&self, harness: &str) -> Vec<Skill> {
        self.skills
            .read()
            .unwrap()
            .values()
            .filter(|s| s.enabled && s.harnesses.iter().any(|h| h == harness))
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_skill(id: &str) -> Skill {
        Skill {
            id: id.to_string(),
            name: format!("Test Skill {}", id),
            version: "1.0.0".to_string(),
            description: "A test skill".to_string(),
            author: Some("test".to_string()),
            source: SkillSource::Builtin,
            enabled: true,
            harnesses: vec!["default".to_string()],
            content: SkillContent::Prompt {
                text: "You are helpful.".to_string(),
            },
            installed_at: "2026-04-04T00:00:00Z".to_string(),
            updated_at: None,
        }
    }

    #[test]
    fn test_install_and_list() {
        let dir = tempfile::tempdir().unwrap();
        let reg = SkillRegistry::with_dir(dir.path().to_path_buf());
        reg.install(make_skill("s1")).unwrap();
        reg.install(make_skill("s2")).unwrap();
        assert_eq!(reg.count(), 2);
        assert_eq!(reg.list().len(), 2);
    }

    #[test]
    fn test_get_skill() {
        let dir = tempfile::tempdir().unwrap();
        let reg = SkillRegistry::with_dir(dir.path().to_path_buf());
        reg.install(make_skill("s1")).unwrap();
        let s = reg.get("s1").unwrap();
        assert_eq!(s.name, "Test Skill s1");
        assert!(reg.get("nonexistent").is_none());
    }

    #[test]
    fn test_toggle() {
        let dir = tempfile::tempdir().unwrap();
        let reg = SkillRegistry::with_dir(dir.path().to_path_buf());
        reg.install(make_skill("s1")).unwrap();
        assert!(reg.get("s1").unwrap().enabled);
        let new_state = reg.toggle("s1").unwrap();
        assert!(!new_state);
        assert!(!reg.get("s1").unwrap().enabled);
        let new_state = reg.toggle("s1").unwrap();
        assert!(new_state);
    }

    #[test]
    fn test_uninstall() {
        let dir = tempfile::tempdir().unwrap();
        let reg = SkillRegistry::with_dir(dir.path().to_path_buf());
        reg.install(make_skill("s1")).unwrap();
        assert_eq!(reg.count(), 1);
        let removed = reg.uninstall("s1").unwrap();
        assert_eq!(removed.id, "s1");
        assert_eq!(reg.count(), 0);
        // Backup file should exist
        assert!(dir.path().join(".backup/s1.json").exists());
    }

    #[test]
    fn test_for_harness() {
        let dir = tempfile::tempdir().unwrap();
        let reg = SkillRegistry::with_dir(dir.path().to_path_buf());

        let mut s1 = make_skill("s1");
        s1.harnesses = vec!["default".to_string(), "code".to_string()];
        reg.install(s1).unwrap();

        let mut s2 = make_skill("s2");
        s2.harnesses = vec!["code".to_string()];
        reg.install(s2).unwrap();

        let mut s3 = make_skill("s3");
        s3.enabled = false;
        s3.harnesses = vec!["default".to_string()];
        reg.install(s3).unwrap();

        assert_eq!(reg.for_harness("default").len(), 1); // s1 only (s3 disabled)
        assert_eq!(reg.for_harness("code").len(), 2); // s1 + s2
        assert_eq!(reg.for_harness("other").len(), 0);
    }

    #[test]
    fn test_toggle_harness() {
        let dir = tempfile::tempdir().unwrap();
        let reg = SkillRegistry::with_dir(dir.path().to_path_buf());
        reg.install(make_skill("s1")).unwrap();
        assert_eq!(reg.get("s1").unwrap().harnesses, vec!["default"]);
        reg.toggle_harness("s1", "code").unwrap();
        let s = reg.get("s1").unwrap();
        assert!(s.harnesses.contains(&"code".to_string()));
        assert!(s.harnesses.contains(&"default".to_string()));
        reg.toggle_harness("s1", "default").unwrap();
        assert_eq!(reg.get("s1").unwrap().harnesses, vec!["code"]);
    }

    #[test]
    fn test_persistence_across_instances() {
        let dir = tempfile::tempdir().unwrap();
        {
            let reg = SkillRegistry::with_dir(dir.path().to_path_buf());
            reg.install(make_skill("s1")).unwrap();
            reg.install(make_skill("s2")).unwrap();
        }
        // New instance should load from disk
        let reg2 = SkillRegistry::with_dir(dir.path().to_path_buf());
        assert_eq!(reg2.count(), 2);
        assert!(reg2.get("s1").is_some());
    }
}
