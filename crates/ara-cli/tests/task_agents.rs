//! Agent discovery/source assertions; authored, not executed in D candidate.
#[path = "../src/task_agents.rs"]
mod task_agents;
use serde_json::json;

#[test]
fn fixed_five_agents_preserve_prompt_roles_thinking_tools_and_outputs(){
    let directory=tempfile::tempdir().unwrap();let project=directory.path().join("project");let agent=directory.path().join("agent");
    std::fs::create_dir_all(&project).unwrap();std::fs::create_dir_all(&agent).unwrap();
    let task=task_agents::load(&project,&agent,"task").unwrap();assert_eq!(task.model.as_deref(),Some("@task"));assert_eq!(task.model_role.as_deref(),Some("task"));assert_eq!(task.thinking.as_deref(),Some("auto"));assert_eq!(task.spawns.as_deref(),Some("*"));assert!(!task.blocking);assert!(!task.read_only);assert!(task.prompt[0].contains("MUST hyperfocus assigned task"));
    let sonic=task_agents::load(&project,&agent,"sonic").unwrap();assert_eq!(sonic.model.as_deref(),Some("@smol"));assert_eq!(sonic.thinking.as_deref(),Some("medium"));assert_eq!(sonic.prompt,task.prompt);assert_eq!(sonic.spawns,None);assert!(!sonic.blocking);
    let scout=task_agents::load(&project,&agent,"scout").unwrap();assert_eq!(scout.model_patterns,["@smol"]);assert_eq!(scout.thinking.as_deref(),Some("medium"));assert_eq!(scout.read_summarize,Some(false));assert!(scout.read_only);assert!(!scout.blocking);assert_eq!(scout.tools.as_ref().unwrap(),&["read","find","grep","glob","web_search"]);assert_eq!(scout.output_schema.as_ref().unwrap()["properties"]["files"]["type"],"array");assert!(scout.output_schema_source.as_ref().unwrap()["properties"]["files"].get("elements").is_some());
    let reviewer=task_agents::load(&project,&agent,"reviewer").unwrap();assert_eq!(reviewer.model.as_deref(),Some("@slow"));assert_eq!(reviewer.model_role.as_deref(),Some("slow"));assert_eq!(reviewer.thinking,None);assert_eq!(reviewer.spawns.as_deref(),Some("scout"));assert!(reviewer.read_only);assert!(reviewer.tools.unwrap().iter().any(|tool|tool=="bash"));assert!(reviewer.prompt[0].contains("Bash read-only:"));
    let security=task_agents::load(&project,&agent,"security-reviewer").unwrap();assert_eq!(security.model,None);assert_eq!(security.thinking,None);assert!(security.read_only);assert!(!security.tools.as_ref().unwrap().iter().any(|tool|tool=="bash"));assert!(security.prompt[0].contains("Do not edit, execute payloads, or make network calls."));
}

#[test]
fn actual_native_roots_override_in_order_and_settings_preserve_model_fallbacks(){
    let directory=tempfile::tempdir().unwrap();let project=directory.path().join("project");let nested=project.join("nested");let agent=directory.path().join("agent");let extension=directory.path().join("extension");
    for path in [project.join(".ara/agents"),agent.join("agents"),extension.join("agents"),nested.clone()]{std::fs::create_dir_all(path).unwrap();}
    let definition=|body:&str|format!("---\nname: reviewer\ndescription: local reviewer\nmodel: [provider/first, provider/fallback]\nthinking-level: med\ntools: [READ, search, grep, task, PluginTool]\nblocking: true\n---\n{body}");
    std::fs::write(extension.join("agents/reviewer.md"),definition("extension prompt")).unwrap();
    std::fs::write(agent.join("agents/reviewer.md"),definition("user prompt")).unwrap();
    std::fs::write(project.join(".ara/agents/reviewer.md"),definition("project prompt")).unwrap();
    let loaded=task_agents::load_with_extension_roots(&nested,&agent,"reviewer",std::slice::from_ref(&extension)).unwrap();assert_eq!(loaded.prompt,["project prompt"]);assert_eq!(loaded.model_patterns,["provider/first","provider/fallback"]);assert_eq!(loaded.thinking.as_deref(),Some("medium"));assert_eq!(loaded.tools.unwrap(),["read","grep","task","PluginTool"]);assert_eq!(loaded.spawns.as_deref(),Some("*"));assert!(loaded.blocking);assert!(!loaded.read_only);
    std::fs::write(agent.join("config.yml"),"task:\n  agentModelOverrides:\n    reviewer: [global/first, global/fallback]\n").unwrap();
    std::fs::create_dir_all(nested.join(".ara")).unwrap();std::fs::write(nested.join(".ara/config.yml"),"task:\n  agentModelOverrides:\n    reviewer: [project/first, project/fallback]\n").unwrap();
    let configured=task_agents::load(&nested,&agent,"reviewer").unwrap();assert_eq!(configured.model_patterns,["project/first","project/fallback"]);assert_eq!(configured.model.as_deref(),Some("project/first"));
    std::fs::write(nested.join(".ara/config.yml"),"task:\n  disabledAgents: [reviewer]\n").unwrap();assert!(task_agents::load(&nested,&agent,"reviewer").unwrap_err().to_string().contains("disabled"));
}

#[test]
fn custom_agent_contract_keeps_empty_tools_invalid_schema_and_reserved_names_explicit(){
    let custom=task_agents::parse("custom.md","---\nname: custom\ndescription: custom\ntools: []\noutput: false\n---\nCustom prompt").unwrap();
    assert_eq!(custom.tools,Some(vec![]));assert_eq!(custom.output_schema,Some(json!(false)));assert!(!custom.warnings.is_empty());assert_eq!(custom.schema_mode,None);
    assert!(task_agents::parse("reserved.md","---\nname: main\ndescription: main impersonation\n---\nbody").is_err());
    let directory=tempfile::tempdir().unwrap();assert!(task_agents::load(directory.path(),directory.path(),"unknown").unwrap_err().to_string().contains("Unknown agent"));
}
