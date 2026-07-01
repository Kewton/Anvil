use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::planner::profile::ProfileQualityExpectations;
use crate::planner::profile::profile_failure;
use crate::planner::verify::{VerificationReport, VerifyStatus};

pub fn generation_rules(intent: &str) -> &'static str {
    match intent {
        "create" => {
            "- Profile nextjs/create: preserve a real Next.js app contract. Include next/react/react-dom dependencies, keep scripts.build as next build, and end with a build verification phase. Put dependency setup before any npm run build verification when node_modules is not already present; setup instructions may install dependencies, but verify must not contain npm install. If dependency setup is not allowed or cannot run, stop with dependency_missing instead of claiming build success. If you use Tailwind utility classes or @tailwind directives, include tailwindcss/postcss/autoprefixer and create tailwind.config.* plus postcss.config.*; otherwise use plain CSS and do not write Tailwind utility classes. If the goal mentions port 3011, keep scripts.dev as next dev -p 3011 or next dev --port 3011.\n"
        }
        "fix" => {
            "- Profile nextjs/fix: preserve the existing Next.js structure and verifier integrity. Do not weaken next/react/react-dom dependencies, scripts.build, app/page, layout, or TypeScript configuration to make a failing verifier pass.\n"
        }
        "research" => {
            "- Profile nextjs/research: inspect the existing app and produce concrete findings. Do not modify source unless the user explicitly asks for fixes.\n"
        }
        _ => {
            "- Profile nextjs: preserve a real Next.js app when present. Keep next/react/react-dom dependencies, scripts.build as next build, app/ or pages/ entrypoints, and a final build verification phase. Keep styling toolchains internally consistent.\n"
        }
    }
}

pub fn verify(root: &Path, goal: &str) -> VerificationReport {
    let project = match locate_project_root(root) {
        Ok(project) => project,
        Err(reason) => return profile_failure(reason),
    };
    let package_path = project.path.join("package.json");
    let Ok(content) = std::fs::read_to_string(&package_path) else {
        return profile_failure(project.rel_path("package.json missing"));
    };
    let Ok(package): Result<Value, _> = serde_json::from_str(&content) else {
        return profile_failure(project.rel_path("package.json invalid"));
    };
    let deps = package.get("dependencies").and_then(Value::as_object);
    for dep in ["next", "react", "react-dom"] {
        if deps.is_none_or(|deps| !deps.contains_key(dep)) {
            return profile_failure(format!("dependency missing: {dep}"));
        }
    }
    if let Some(reason) = dependency_coherence_failure(&package) {
        return profile_failure(reason);
    }
    let scripts = package.get("scripts").and_then(Value::as_object);
    let build = scripts
        .and_then(|scripts| scripts.get("build"))
        .and_then(Value::as_str);
    if build != Some("next build") || build.is_some_and(is_weakened_script) {
        return profile_failure("scripts.build must be next build");
    }
    if scripts
        .and_then(|scripts| scripts.get("dev"))
        .and_then(Value::as_str)
        .is_some_and(is_weakened_script)
    {
        return profile_failure("scripts.dev must run next dev");
    }
    if goal.contains("3011") {
        let dev = scripts
            .and_then(|scripts| scripts.get("dev"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if !(dev.contains("next dev") && (dev.contains("-p 3011") || dev.contains("--port 3011"))) {
            return profile_failure("dev script must run next dev on port 3011");
        }
    }
    let Some(entry) = find_entrypoint(&project.path) else {
        return profile_failure(project.rel_path(
            "Next entrypoint missing: expected src/app/page.tsx, app/page.tsx, or pages/index.tsx",
        ));
    };
    if entry.requires_layout && find_app_layout(&project.path, &entry.app_dir).is_none() {
        return profile_failure(project.rel_path(&format!(
            "Next app router layout missing: expected {}/layout.tsx or layout.jsx",
            entry.app_dir
        )));
    }
    let uses_alias = contains_in_files(&project.path, "@/");
    if uses_alias {
        let Ok(tsconfig) = std::fs::read_to_string(project.path.join("tsconfig.json")) else {
            return profile_failure(project.rel_path("tsconfig.json missing for @/* alias"));
        };
        let Ok(tsconfig): Result<Value, _> = serde_json::from_str(&tsconfig) else {
            return profile_failure("tsconfig.json invalid");
        };
        if !alias_configured(&tsconfig) {
            return profile_failure("tsconfig baseUrl/paths missing @/* alias");
        }
    }
    if let Some(reason) = tsconfig_contract_failure(&project.path) {
        return profile_failure(reason);
    }
    if let Some(reason) = css_side_effect_import_contract_failure(&project.path) {
        return profile_failure(reason);
    }
    if let Some(reason) = client_component_contract_failure(&project.path) {
        return profile_failure(reason);
    }
    if let Some(reason) = tailwind_contract_failure(&project.path, &package) {
        return profile_failure(reason);
    }
    VerificationReport::pass()
}

pub fn verify_invariant(root: &Path, goal: &str) -> VerificationReport {
    let project = match locate_project_root(root) {
        Ok(project) => project,
        Err(reason) if reason == "package.json missing" => return VerificationReport::pass(),
        Err(reason) => return profile_failure(reason),
    };
    let package_path = project.path.join("package.json");
    let Ok(content) = std::fs::read_to_string(&package_path) else {
        return profile_failure(project.rel_path("package.json unreadable"));
    };
    let Ok(package): Result<Value, _> = serde_json::from_str(&content) else {
        return profile_failure(project.rel_path("package.json invalid"));
    };
    let deps = package.get("dependencies").and_then(Value::as_object);
    for dep in ["next", "react", "react-dom"] {
        if deps.is_none_or(|deps| !deps.contains_key(dep)) {
            return profile_failure(format!("dependency missing: {dep}"));
        }
    }
    if let Some(reason) = dependency_coherence_failure(&package) {
        return profile_failure(reason);
    }
    let scripts = package.get("scripts").and_then(Value::as_object);
    let build = scripts
        .and_then(|scripts| scripts.get("build"))
        .and_then(Value::as_str);
    if build.is_some_and(|build| build != "next build" || is_weakened_script(build)) {
        return profile_failure("scripts.build must be next build");
    }
    if goal.contains("3011") {
        let dev = scripts
            .and_then(|scripts| scripts.get("dev"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if !dev.is_empty()
            && !(dev.contains("next dev")
                && (dev.contains("-p 3011") || dev.contains("--port 3011")))
        {
            return profile_failure("dev script must run next dev on port 3011");
        }
    }
    if let Some(reason) = tsconfig_contract_failure(&project.path) {
        return profile_failure(reason);
    }
    if let Some(reason) = css_side_effect_import_contract_failure(&project.path) {
        return profile_failure(reason);
    }
    if let Some(reason) = client_component_contract_failure(&project.path) {
        return profile_failure(reason);
    }
    if let Some(reason) = tailwind_contract_failure(&project.path, &package) {
        return profile_failure(reason);
    }
    VerificationReport::pass()
}

pub fn guidance(goal: &str) -> String {
    let port = if goal.contains("3011") {
        " The dev script must run `next dev -p 3011` or `next dev --port 3011`."
    } else {
        ""
    };
    format!(
        "For the nextjs profile, create a runnable Next.js app, not only package metadata. \
         Keep the project in the workspace root unless a project subdirectory already exists. \
         Required artifacts by completion: package.json, src/app/page.tsx, src/app/layout.tsx, src/app/global.d.ts. \
         If those files are absent, write package.json, src/app/layout.tsx, src/app/page.tsx, and src/app/global.d.ts before further inspection. \
         If any layout imports CSS such as ./globals.css, src/app/global.d.ts must declare module \"*.css\". \
         package.json must include compatible next, react, react-dom, @types/react, @types/react-dom, and TypeScript 5.x dependencies plus scripts.build = `next build`. \
         For TypeScript/TSX apps, create tsconfig.json before treating the app as complete. \
         Do not use deprecated moduleResolution=node10 or target=ES5; prefer moduleResolution=bundler and target=ES2017 or newer.{port}"
    )
}

pub fn runtime_contract(intent: &str, goal: &str) -> String {
    let port = if goal.contains("3011") {
        "\n- If a 3011 port requirement exists, keep scripts.dev as next dev -p 3011 or next dev --port 3011."
    } else {
        ""
    };
    match intent {
        "create" => format!(
            "- Preserve the workspace as a real Next.js app.\n\
- Keep next/react/react-dom dependencies in package.json.\n\
- Keep scripts.build as next build; do not replace it with echo/skip/no-op commands.\n\
- If npm run build cannot run because dependencies are not installed, report dependency_missing or use an explicit setup step; do not fake success.\
{port}\n\
- If using Tailwind utility classes or @tailwind directives, keep the Tailwind toolchain complete. Otherwise use plain CSS.\n\
- Keep TypeScript and app router configuration coherent.\n\
- Do not treat scaffold-only, package-only, or build-only output as complete."
        ),
        "fix" => format!(
            "- Preserve the existing Next.js app structure.\n\
- Keep next/react/react-dom dependencies when already present.\n\
- Keep scripts.build as next build when already present; do not weaken build/test scripts to hide failures.\n\
- If npm run build cannot run because dependencies are missing, report dependency_missing or use the existing dependency workflow.\
{port}\n\
- Keep TypeScript and app router configuration coherent.\n\
- Do not treat scaffold-only, package-only, or build-only output as complete."
        ),
        "research" | "investigate" => {
            "- Preserve the existing Next.js app unchanged unless the phase explicitly asks for fixes.\n\
- Produce concrete findings from inspected files and commands.\n\
- Separate observed facts from hypotheses.\n\
- Do not weaken package scripts or test/build checks while investigating."
                .to_string()
        }
        _ => format!(
            "- Preserve the workspace as a Next.js app when one exists.\n\
- Do not convert package.json to a standalone TypeScript/Node project.\n\
- Keep next/react/react-dom dependencies when already present.\n\
- Keep scripts.build as next build when already present.\
{port}\n\
- Keep styling and TypeScript toolchains internally consistent.\n\
- Do not treat scaffold-only, package-only, or build-only output as complete."
        ),
    }
}

pub fn expected_paths(root: &Path, goal: &str) -> Vec<String> {
    let prefix = existing_project_prefix(root);
    let mut paths = vec![format!("{prefix}package.json")];
    if !goal.to_ascii_lowercase().contains("scaffold") {
        paths.push(format!("{prefix}src/app/page.tsx"));
        paths.push(format!("{prefix}src/app/layout.tsx"));
        paths.push(format!("{prefix}src/app/global.d.ts"));
    }
    paths
}

pub fn quality_expectations(root: &Path, goal: &str) -> ProfileQualityExpectations {
    ProfileQualityExpectations {
        required_artifacts: expected_paths(root, goal),
        preferred_verify: vec!["npm run build".to_string()],
        forbidden_verify: vec![
            "next dev".to_string(),
            "npm install".to_string(),
            "pnpm install".to_string(),
            "yarn install".to_string(),
        ],
        dependency_order_hint: Some(
            "Create package.json and a Next.js entrypoint before npm run build".to_string(),
        ),
    }
}

pub fn repair_prompt(root: &Path, goal: &str, report: &VerificationReport) -> String {
    let expected = expected_paths(root, goal).join(", ");
    let failure = match &report.status {
        VerifyStatus::ProfileContractFailed(reason) => reason.as_str(),
        _ => "profile verification failed",
    };
    format!(
        "Repair the Next.js profile contract for this goal: {goal}\n\
         Failure: {failure}\n\
         Required paths: {expected}\n\
         Make the smallest bounded change inside the workspace. \
         If package.json exists only in a project subdirectory, continue using that subdirectory. \
         Ensure the app has a concrete playable page and layout, package dependencies, \
         scripts.build = `next build`, and a dev script on port 3011 when the goal mentions 3011. \
         Use tools for file changes, then stop."
    )
}

pub fn auto_repair(root: &Path, goal: &str, report: &VerificationReport) -> anyhow::Result<bool> {
    if report.is_pass() {
        return Ok(false);
    }
    let project = locate_project_root(root).unwrap_or_else(|_| ProjectRoot {
        path: root.to_path_buf(),
        prefix: String::new(),
    });
    std::fs::create_dir_all(project.path.join("src/app"))?;
    ensure_package_json(&project.path, goal)?;
    ensure_file(
        &project.path.join("next.config.js"),
        "/** @type {import('next').NextConfig} */\nconst nextConfig = {};\n\nmodule.exports = nextConfig;\n",
    )?;
    ensure_file(
        &project.path.join("tsconfig.json"),
        r#"{"compilerOptions":{"target":"ES2017","lib":["dom","dom.iterable","esnext"],"allowJs":true,"skipLibCheck":true,"strict":true,"noEmit":true,"esModuleInterop":true,"module":"esnext","moduleResolution":"bundler","resolveJsonModule":true,"isolatedModules":true,"jsx":"preserve","incremental":true,"plugins":[{"name":"next"}],"baseUrl":".","paths":{"@/*":["./src/*"]}},"include":["next-env.d.ts","**/*.ts","**/*.tsx",".next/types/**/*.ts"],"exclude":["node_modules"]}"#,
    )?;
    ensure_file(
        &project.path.join("src/app/globals.css"),
        "* { box-sizing: border-box; }\nhtml, body { margin: 0; min-height: 100%; background: #05070d; color: #eef7ff; }\nbutton { font: inherit; }\n",
    )?;
    ensure_file(
        &project.path.join("src/app/global.d.ts"),
        "declare module \"*.css\";\n",
    )?;
    ensure_file(
        &project.path.join("src/app/layout.tsx"),
        r#"import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "Neon Space Invaders",
  description: "A compact arcade shooter generated by anvilminimal",
};

export default function RootLayout({
  children,
}: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
"#,
    )?;
    ensure_file(&project.path.join("src/app/page.tsx"), fallback_page())?;
    Ok(true)
}

pub fn repair_manifest_coherence(root: &Path, goal: &str) -> anyhow::Result<bool> {
    let Ok(project) = locate_project_root(root) else {
        return Ok(false);
    };
    let path = project.path.join("package.json");
    if !path.is_file() {
        return Ok(false);
    }
    let before = std::fs::read_to_string(&path).unwrap_or_default();
    ensure_package_json(&project.path, goal)?;
    let after = std::fs::read_to_string(&path).unwrap_or_default();
    Ok(before != after)
}

#[derive(Debug, Clone)]
struct ProjectRoot {
    path: PathBuf,
    prefix: String,
}

impl ProjectRoot {
    fn rel_path(&self, message: &str) -> String {
        if self.prefix.is_empty() {
            message.to_string()
        } else {
            format!("{}: {message}", self.prefix.trim_end_matches('/'))
        }
    }
}

#[derive(Debug, Clone)]
struct EntryPoint {
    app_dir: String,
    requires_layout: bool,
}

fn locate_project_root(root: &Path) -> Result<ProjectRoot, String> {
    if root.join("package.json").is_file() {
        return Ok(ProjectRoot {
            path: root.to_path_buf(),
            prefix: String::new(),
        });
    }
    let mut nested = Vec::new();
    let entries = std::fs::read_dir(root).map_err(|_| "package.json missing".to_string())?;
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() || entry.path().join("node_modules").is_dir() {
            continue;
        }
        if entry.path().join("package.json").is_file() {
            let name = entry.file_name().to_string_lossy().to_string();
            nested.push(ProjectRoot {
                path: entry.path(),
                prefix: format!("{name}/"),
            });
        }
    }
    match nested.len() {
        0 => Err("package.json missing".to_string()),
        1 => Ok(nested.remove(0)),
        _ => Err(
            "multiple nested package.json files found; keep one Next.js project in the workspace"
                .to_string(),
        ),
    }
}

fn existing_project_prefix(root: &Path) -> String {
    locate_project_root(root)
        .map(|project| project.prefix)
        .unwrap_or_default()
}

fn ensure_package_json(root: &Path, goal: &str) -> anyhow::Result<()> {
    let path = root.join("package.json");
    let mut package = std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    package
        .entry("name")
        .or_insert_with(|| Value::String("anvilminimal-nextjs-app".to_string()));
    package
        .entry("version")
        .or_insert_with(|| Value::String("1.0.0".to_string()));
    package
        .entry("private")
        .or_insert_with(|| Value::Bool(true));
    let deps = object_entry(&mut package, "dependencies");
    ensure_dependency(deps, "next", "^14.2.0");
    ensure_dependency(deps, "react", "^18.3.0");
    ensure_dependency(deps, "react-dom", "^18.3.0");
    let tailwind_used = uses_tailwind(root, &Value::Object(package.clone()));
    let dev_deps = object_entry(&mut package, "devDependencies");
    ensure_dependency(dev_deps, "typescript", "^5.5.0");
    ensure_dependency(dev_deps, "@types/node", "^20.14.0");
    ensure_dependency(dev_deps, "@types/react", "^18.3.0");
    ensure_dependency(dev_deps, "@types/react-dom", "^18.3.0");
    if tailwind_used {
        ensure_dependency(dev_deps, "tailwindcss", "^3.4.19");
        ensure_dependency(dev_deps, "postcss", "^8.5.15");
        ensure_dependency(dev_deps, "autoprefixer", "^10.4.20");
    }
    let scripts = object_entry(&mut package, "scripts");
    let dev = if goal.contains("3011") {
        "next dev -p 3011"
    } else {
        "next dev"
    };
    scripts.insert("dev".to_string(), Value::String(dev.to_string()));
    scripts.insert("build".to_string(), Value::String("next build".to_string()));
    scripts.insert(
        "start".to_string(),
        Value::String(if goal.contains("3011") {
            "next start -p 3011".to_string()
        } else {
            "next start".to_string()
        }),
    );
    let content = serde_json::to_string_pretty(&Value::Object(package))?;
    std::fs::write(path, format!("{content}\n"))?;
    Ok(())
}

fn object_entry<'a>(package: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    let value = package
        .entry(key.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }
    value.as_object_mut().expect("value was just made object")
}

fn ensure_dependency(deps: &mut Map<String, Value>, name: &str, version: &str) {
    let needs_update = deps
        .get(name)
        .and_then(Value::as_str)
        .is_none_or(|current| dependency_version_needs_repair(name, current));
    if needs_update {
        deps.insert(name.to_string(), Value::String(version.to_string()));
    }
}

fn ensure_file(path: &Path, content: &str) -> anyhow::Result<()> {
    if !path.is_file() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
    }
    Ok(())
}

fn fallback_page() -> &'static str {
    r#""use client";

import { useEffect, useMemo, useState } from "react";

type Invader = { id: number; x: number; y: number; alive: boolean };
type Shot = { x: number; y: number };

const columns = 9;
const rows = 4;

function initialInvaders(): Invader[] {
  return Array.from({ length: columns * rows }, (_, id) => ({
    id,
    x: 8 + (id % columns) * 10,
    y: 12 + Math.floor(id / columns) * 8,
    alive: true,
  }));
}

export default function Page() {
  const [ship, setShip] = useState(50);
  const [shots, setShots] = useState<Shot[]>([]);
  const [invaders, setInvaders] = useState<Invader[]>(() => initialInvaders());
  const [tick, setTick] = useState(0);
  const [running, setRunning] = useState(true);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") setShip((value) => Math.max(5, value - 4));
      if (event.key === "ArrowRight") setShip((value) => Math.min(95, value + 4));
      if (event.key === " ") setShots((value) => [...value, { x: ship, y: 86 }].slice(-6));
      if (event.key.toLowerCase() === "r") {
        setInvaders(initialInvaders());
        setShots([]);
        setRunning(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [ship]);

  useEffect(() => {
    if (!running) return;
    const timer = window.setInterval(() => {
      setTick((value) => value + 1);
      setShots((value) => value.map((shot) => ({ ...shot, y: shot.y - 5 })).filter((shot) => shot.y > 4));
      setInvaders((value) =>
        value.map((invader) => ({
          ...invader,
          x: invader.x + Math.sin((tick + invader.id) / 8) * 0.45,
          y: invader.y + 0.035,
        })),
      );
    }, 70);
    return () => window.clearInterval(timer);
  }, [running, tick]);

  useEffect(() => {
    setInvaders((current) =>
      current.map((invader) => {
        if (!invader.alive) return invader;
        const hit = shots.some((shot) => Math.abs(shot.x - invader.x) < 3.2 && Math.abs(shot.y - invader.y) < 3.8);
        return hit ? { ...invader, alive: false } : invader;
      }),
    );
  }, [shots]);

  const alive = invaders.filter((invader) => invader.alive).length;
  const score = useMemo(() => (columns * rows - alive) * 100, [alive]);

  useEffect(() => {
    if (alive === 0 || invaders.some((invader) => invader.alive && invader.y > 78)) {
      setRunning(false);
    }
  }, [alive, invaders]);

  return (
    <main className="screen">
      <section className="hud">
        <strong>NEON INVADERS</strong>
        <span>SCORE {score}</span>
        <span>{running ? "LIVE" : alive === 0 ? "CLEAR" : "BREACH"}</span>
      </section>
      <section className="arena" aria-label="Space invaders play field">
        <div className="stars" />
        {invaders.map((invader) =>
          invader.alive ? (
            <div
              className="invader"
              key={invader.id}
              style={{ left: `${invader.x}%`, top: `${invader.y}%` }}
            />
          ) : null,
        )}
        {shots.map((shot, index) => (
          <div className="shot" key={`${shot.x}-${shot.y}-${index}`} style={{ left: `${shot.x}%`, top: `${shot.y}%` }} />
        ))}
        <div className="ship" style={{ left: `${ship}%` }} />
      </section>
      <nav className="controls">
        <button onClick={() => setShip((value) => Math.max(5, value - 5))}>Left</button>
        <button onClick={() => setShots((value) => [...value, { x: ship, y: 86 }].slice(-6))}>Fire</button>
        <button onClick={() => setShip((value) => Math.min(95, value + 5))}>Right</button>
        <button
          onClick={() => {
            setInvaders(initialInvaders());
            setShots([]);
            setRunning(true);
          }}
        >
          Reset
        </button>
      </nav>
      <style jsx>{`
        .screen {
          min-height: 100vh;
          padding: 24px;
          display: grid;
          grid-template-rows: auto 1fr auto;
          gap: 16px;
          background: #05070d;
          color: #edfaff;
          font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
        }
        .hud, .controls {
          display: flex;
          justify-content: center;
          gap: 12px;
          flex-wrap: wrap;
        }
        .hud span, .hud strong, .controls button {
          border: 1px solid rgba(129, 245, 255, 0.45);
          background: rgba(5, 12, 24, 0.72);
          color: #effcff;
          padding: 10px 14px;
          border-radius: 6px;
          box-shadow: 0 0 18px rgba(0, 229, 255, 0.16);
        }
        .controls button { cursor: pointer; min-width: 84px; }
        .arena {
          position: relative;
          overflow: hidden;
          min-height: 560px;
          border: 1px solid rgba(129, 245, 255, 0.36);
          background: rgba(2, 4, 12, 0.84);
          box-shadow: inset 0 0 70px rgba(0, 229, 255, 0.12);
        }
        .stars {
          position: absolute;
          inset: 0;
          background-image: radial-gradient(#fff 1px, transparent 1px);
          background-size: 31px 29px;
          opacity: 0.2;
        }
        .invader, .shot, .ship { position: absolute; transform: translate(-50%, -50%); }
        .invader {
          width: 24px;
          height: 18px;
          background: #7dffbf;
          clip-path: polygon(12% 0, 88% 0, 100% 35%, 70% 35%, 70% 70%, 88% 70%, 88% 100%, 60% 78%, 40% 78%, 12% 100%, 12% 70%, 30% 70%, 30% 35%, 0 35%);
          filter: drop-shadow(0 0 12px #7dffbf);
        }
        .shot {
          width: 4px;
          height: 20px;
          border-radius: 999px;
          background: #ffec7d;
          box-shadow: 0 0 14px #ffec7d;
        }
        .ship {
          bottom: 28px;
          width: 46px;
          height: 30px;
          background: #7dc7ff;
          clip-path: polygon(50% 0, 100% 100%, 66% 82%, 34% 82%, 0 100%);
          filter: drop-shadow(0 0 16px #7dc7ff);
        }
      `}</style>
    </main>
  );
}
"#
}

fn find_entrypoint(root: &Path) -> Option<EntryPoint> {
    for (rel, app_dir) in [
        ("src/app/page.tsx", "src/app"),
        ("src/app/page.jsx", "src/app"),
        ("src/app/page.ts", "src/app"),
        ("src/app/page.js", "src/app"),
        ("app/page.tsx", "app"),
        ("app/page.jsx", "app"),
        ("app/page.ts", "app"),
        ("app/page.js", "app"),
    ] {
        if root.join(rel).is_file() {
            return Some(EntryPoint {
                app_dir: app_dir.to_string(),
                requires_layout: true,
            });
        }
    }
    for rel in [
        "pages/index.tsx",
        "pages/index.jsx",
        "pages/index.ts",
        "pages/index.js",
        "src/pages/index.tsx",
        "src/pages/index.jsx",
        "src/pages/index.ts",
        "src/pages/index.js",
    ] {
        if root.join(rel).is_file() {
            return Some(EntryPoint {
                app_dir: String::new(),
                requires_layout: false,
            });
        }
    }
    None
}

fn find_app_layout(root: &Path, app_dir: &str) -> Option<PathBuf> {
    ["layout.tsx", "layout.jsx", "layout.ts", "layout.js"]
        .iter()
        .map(|name| root.join(app_dir).join(name))
        .find(|path| path.is_file())
}

fn contains_in_files(root: &Path, needle: &str) -> bool {
    for rel in [
        "app/page.tsx",
        "app/page.jsx",
        "pages/index.tsx",
        "pages/index.jsx",
        "src/app/page.tsx",
        "src/app/page.jsx",
        "src/app/globals.css",
        "app/globals.css",
        "src/pages/index.tsx",
        "src/pages/index.jsx",
    ] {
        if std::fs::read_to_string(root.join(rel)).is_ok_and(|content| content.contains(needle)) {
            return true;
        }
    }
    false
}

fn is_weakened_script(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    value.is_empty()
        || value == "true"
        || value == "echo ok"
        || value == "echo done"
        || value.starts_with("echo ")
}

fn tsconfig_contract_failure(root: &Path) -> Option<String> {
    let path = root.join("tsconfig.json");
    if !path.is_file() {
        return None;
    }
    let content = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&content).ok()?;
    let compiler = value.get("compilerOptions").and_then(Value::as_object)?;
    if compiler
        .get("moduleResolution")
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case("node10"))
    {
        return Some(
            "tsconfig.moduleResolution=node10 is deprecated for Next.js builds; use bundler or node16"
                .to_string(),
        );
    }
    let root_dir = compiler.get("rootDir").and_then(Value::as_str)?;
    if !matches!(root_dir, "." | "./") {
        Some("tsconfig.rootDir must not constrain Next.js generated files".to_string())
    } else {
        None
    }
}

fn css_side_effect_import_contract_failure(root: &Path) -> Option<String> {
    let imports_css = [
        "src/app/layout.tsx",
        "src/app/layout.ts",
        "app/layout.tsx",
        "app/layout.ts",
    ]
    .iter()
    .any(|rel| {
        std::fs::read_to_string(root.join(rel))
            .is_ok_and(|content| content.contains(".css\"") || content.contains(".css'"))
    });
    if !imports_css {
        return None;
    }
    if css_module_declaration_exists(root) {
        None
    } else {
        Some(
            "CSS side-effect imports require a declaration file such as src/app/global.d.ts with declare module \"*.css\""
                .to_string(),
        )
    }
}

fn css_module_declaration_exists(root: &Path) -> bool {
    for rel in [
        "src/app/global.d.ts",
        "src/global.d.ts",
        "global.d.ts",
        "app/global.d.ts",
    ] {
        if std::fs::read_to_string(root.join(rel)).is_ok_and(|content| {
            content.contains("declare module \"*.css\"")
                || content.contains("declare module '*.css'")
        }) {
            return true;
        }
    }
    false
}

fn alias_configured(tsconfig: &Value) -> bool {
    let Some(compiler) = tsconfig.get("compilerOptions").and_then(Value::as_object) else {
        return false;
    };
    let Some(base_url) = compiler.get("baseUrl").and_then(Value::as_str) else {
        return false;
    };
    if !matches!(base_url, "." | "./") {
        return false;
    }
    compiler
        .get("paths")
        .and_then(Value::as_object)
        .and_then(|paths| paths.get("@/*"))
        .and_then(Value::as_array)
        .is_some_and(|values| {
            values.iter().any(|value| {
                matches!(
                    value.as_str(),
                    Some("./src/*") | Some("src/*") | Some("./*") | Some("*")
                )
            })
        })
}

fn tailwind_contract_failure(root: &Path, package: &Value) -> Option<String> {
    if !uses_tailwind(root, package) {
        return None;
    }
    for dep in ["tailwindcss", "postcss", "autoprefixer"] {
        if !package_has_dependency(package, dep) {
            return Some(tailwind_failure(format!(
                "Tailwind toolchain dependency missing: {dep}"
            )));
        }
    }
    if !has_tailwind_config(root) {
        return Some(tailwind_failure("Tailwind config file missing"));
    }
    let Some(postcss_config) = postcss_config_path(root) else {
        return Some(tailwind_failure("PostCSS config file missing for Tailwind"));
    };
    let postcss_config = std::fs::read_to_string(postcss_config).unwrap_or_default();
    let postcss_lower = postcss_config.to_ascii_lowercase();
    if !(postcss_lower.contains("tailwindcss") || postcss_lower.contains("@tailwindcss/postcss")) {
        return Some(tailwind_failure(
            "PostCSS config must include the Tailwind plugin",
        ));
    }
    if !postcss_lower.contains("autoprefixer") {
        return Some(tailwind_failure(
            "PostCSS config must include autoprefixer for Tailwind",
        ));
    }
    let tailwind_css_files = tailwind_directive_files(root);
    if !tailwind_css_files.is_empty() {
        let imported = imported_app_css_paths(root);
        if !tailwind_css_files
            .iter()
            .any(|path| imported.iter().any(|imported| imported == path))
        {
            let css_list = tailwind_css_files
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Some(tailwind_failure(format!(
                "@tailwind CSS file must be imported by app layout: {css_list}"
            )));
        }
    }
    None
}

fn tailwind_failure(message: impl AsRef<str>) -> String {
    format!("tailwind_contract_failure: {}", message.as_ref())
}

fn client_component_contract_failure(root: &Path) -> Option<String> {
    for rel in [
        "src/app/page.tsx",
        "src/app/page.jsx",
        "src/app/page.ts",
        "src/app/page.js",
        "app/page.tsx",
        "app/page.jsx",
        "app/page.ts",
        "app/page.js",
    ] {
        let Ok(content) = std::fs::read_to_string(root.join(rel)) else {
            continue;
        };
        if uses_client_only_features(&content) && !has_use_client_directive(&content) {
            return Some(format!(
                "{rel} uses browser/client APIs and must start with \"use client\""
            ));
        }
    }
    None
}

fn uses_client_only_features(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    [
        "usestate",
        "useeffect",
        "useref",
        "usereducer",
        "window.",
        "document.",
        "addeventlistener",
        "requestanimationframe",
        "setinterval",
        "settimeout",
        "onclick=",
        "onkeydown=",
        "onkeyup=",
        "onpointer",
        "onmouse",
        "ref={",
        "<canvas",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn has_use_client_directive(content: &str) -> bool {
    content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("//"))
        .map(|line| {
            let line = line.strip_suffix(';').unwrap_or(line);
            matches!(line, "\"use client\"" | "'use client'")
        })
        .unwrap_or(false)
}

fn uses_tailwind(root: &Path, package: &Value) -> bool {
    package_has_dependency(package, "tailwindcss")
        || !tailwind_directive_files(root).is_empty()
        || has_tailwind_config(root)
        || postcss_config_references_tailwind(root)
}

fn has_tailwind_config(root: &Path) -> bool {
    [
        "tailwind.config.js",
        "tailwind.config.cjs",
        "tailwind.config.mjs",
        "tailwind.config.ts",
    ]
    .iter()
    .any(|rel| root.join(rel).is_file())
}

fn postcss_config_references_tailwind(root: &Path) -> bool {
    let Some(path) = postcss_config_path(root) else {
        return false;
    };
    std::fs::read_to_string(path)
        .is_ok_and(|content| content.to_ascii_lowercase().contains("tailwind"))
}

fn postcss_config_path(root: &Path) -> Option<PathBuf> {
    [
        "postcss.config.js",
        "postcss.config.mjs",
        "postcss.config.cjs",
    ]
    .iter()
    .map(|rel| root.join(rel))
    .find(|path| path.is_file())
}

fn tailwind_directive_files(root: &Path) -> Vec<PathBuf> {
    [
        "src/app/globals.css",
        "src/app/global.css",
        "app/globals.css",
        "app/global.css",
        "src/styles/globals.css",
        "styles/globals.css",
    ]
    .iter()
    .filter_map(|rel| {
        let path = root.join(rel);
        std::fs::read_to_string(&path)
            .ok()
            .filter(|content| content.contains("@tailwind"))
            .map(|_| path)
    })
    .collect()
}

fn imported_app_css_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for rel in [
        "src/app/layout.tsx",
        "src/app/layout.jsx",
        "src/app/layout.ts",
        "src/app/layout.js",
        "app/layout.tsx",
        "app/layout.jsx",
        "app/layout.ts",
        "app/layout.js",
    ] {
        let layout_path = root.join(rel);
        let Ok(content) = std::fs::read_to_string(&layout_path) else {
            continue;
        };
        let layout_dir = layout_path.parent().unwrap_or(root);
        for import in css_imports_from_content(&content) {
            let path = layout_dir
                .join(import.trim_start_matches("./"))
                .components()
                .collect::<PathBuf>();
            paths.push(path);
        }
    }
    paths
}

fn css_imports_from_content(content: &str) -> Vec<String> {
    let mut imports = Vec::new();
    for quote in ['"', '\''] {
        let mut parts = content.split(quote);
        while let Some(_) = parts.next() {
            let Some(candidate) = parts.next() else {
                break;
            };
            if candidate.ends_with(".css") {
                imports.push(candidate.to_string());
            }
        }
    }
    imports
}

fn package_has_dependency(package: &Value, name: &str) -> bool {
    ["dependencies", "devDependencies"]
        .iter()
        .filter_map(|key| package.get(*key).and_then(Value::as_object))
        .any(|deps| deps.contains_key(name))
}

fn dependency_coherence_failure(package: &Value) -> Option<String> {
    let next = dependency_version(package, "next")?;
    let react = dependency_version(package, "react")?;
    let react_dom = dependency_version(package, "react-dom")?;
    if dependency_version_needs_repair(
        "typescript",
        dependency_version(package, "typescript").unwrap_or(""),
    ) {
        return Some(
            "typescript dependency must use a deterministic 5.x range such as ^5.5.0".to_string(),
        );
    }
    let next_major = semver_major(next)?;
    let react_major = semver_major(react)?;
    let react_dom_major = semver_major(react_dom)?;
    if next_major >= 15 && (react_major < 19 || react_dom_major < 19) {
        return Some("Next 15+ requires React/React DOM 19.x compatibility".to_string());
    }
    if next_major <= 14 && (react_major != 18 || react_dom_major != 18) {
        return Some("Next 14 profile expects React/React DOM 18.x compatibility".to_string());
    }
    if let Some(types_react) = dependency_version(package, "@types/react")
        && let Some(types_major) = semver_major(types_react)
        && ((react_major >= 19 && types_major < 19) || (react_major == 18 && types_major != 18))
    {
        return Some("@types/react major must match React major".to_string());
    }
    if let Some(types_react_dom) = dependency_version(package, "@types/react-dom")
        && let Some(types_major) = semver_major(types_react_dom)
        && ((react_dom_major >= 19 && types_major < 19)
            || (react_dom_major == 18 && types_major != 18))
    {
        return Some("@types/react-dom major must match React DOM major".to_string());
    }
    None
}

fn dependency_version<'a>(package: &'a Value, name: &str) -> Option<&'a str> {
    ["dependencies", "devDependencies"]
        .iter()
        .filter_map(|key| package.get(*key).and_then(Value::as_object))
        .find_map(|deps| deps.get(name).and_then(Value::as_str))
}

fn dependency_version_needs_repair(name: &str, version: &str) -> bool {
    if version.trim().is_empty() {
        return true;
    }
    match name {
        "typescript" => {
            let Some(major) = semver_major(version) else {
                return false;
            };
            major != 5 || version.trim() == "5.0.0"
        }
        "@types/node" => semver_major(version).is_none_or(|major| major != 20),
        "@types/react" | "@types/react-dom" => {
            semver_major(version).is_none_or(|major| major != 18)
        }
        "next" => semver_major(version).is_none_or(|major| major != 14),
        "react" | "react-dom" => semver_major(version).is_none_or(|major| major != 18),
        _ => false,
    }
}

fn semver_major(version: &str) -> Option<u64> {
    let trimmed = version.trim();
    let digits = trimmed
        .trim_start_matches(['^', '~', '=', 'v'])
        .split(|ch: char| !ch.is_ascii_digit())
        .next()
        .unwrap_or_default();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::verify::VerifyStatus;

    fn package_json() -> &'static str {
        r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#
    }

    #[test]
    fn nextjs_3011_port_required() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"x","react":"x","react-dom":"x"},"scripts":{"build":"next build","dev":"next dev"}}"#,
        )
        .unwrap();
        assert!(matches!(
            verify(dir.path(), "3011").status,
            VerifyStatus::ProfileContractFailed(_)
        ));
    }

    #[test]
    fn nextjs_requires_entrypoint() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), package_json()).unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("entrypoint")
        ));
    }

    #[test]
    fn nextjs_invariant_allows_pending_entrypoint() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), package_json()).unwrap();
        assert!(verify_invariant(dir.path(), "3011").is_pass());
    }

    #[test]
    fn nextjs_invariant_rejects_weakened_build_script() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"},"scripts":{"build":"echo ok","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        let report = verify_invariant(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("scripts.build")
        ));
    }

    #[test]
    fn nextjs_accepts_single_nested_complete_project() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("space-invaders");
        std::fs::create_dir_all(app.join("src/app")).unwrap();
        std::fs::write(app.join("package.json"), package_json()).unwrap();
        std::fs::write(
            app.join("src/app/page.tsx"),
            "export default function Page(){return null;}",
        )
        .unwrap();
        std::fs::write(app.join("src/app/layout.tsx"), "export default function Layout({children}:{children:React.ReactNode}){return children;}").unwrap();
        assert!(verify(dir.path(), "3011").is_pass());
    }

    #[test]
    fn expected_paths_follow_existing_nested_project() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("space-invaders");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("package.json"), package_json()).unwrap();
        assert_eq!(
            expected_paths(dir.path(), "Implement game"),
            vec![
                "space-invaders/package.json",
                "space-invaders/src/app/page.tsx",
                "space-invaders/src/app/layout.tsx",
                "space-invaders/src/app/global.d.ts"
            ]
        );
    }

    #[test]
    fn nextjs_rejects_missing_css_declaration_for_global_import() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(dir.path().join("package.json"), package_json()).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page(){return null;}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/layout.tsx"),
            "import './globals.css';\nexport default function Layout({children}:{children:React.ReactNode}){return children;}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/globals.css"),
            "body { margin: 0; }",
        )
        .unwrap();

        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("declare module")
        ));

        std::fs::write(
            dir.path().join("src/app/global.d.ts"),
            "declare module \"*.css\";\n",
        )
        .unwrap();
        assert!(verify(dir.path(), "3011").is_pass());
    }

    #[test]
    fn nextjs_rejects_script_weakening() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"},"scripts":{"build":"echo ok","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page(){return null;}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/layout.tsx"),
            "export default function Layout({children}:{children:React.ReactNode}){return children;}",
        )
        .unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("scripts.build")
        ));
    }

    #[test]
    fn nextjs_rejects_tsconfig_rootdir_that_breaks_next() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"rootDir":"src"}}"#,
        )
        .unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("rootDir")
        ));
    }

    #[test]
    fn nextjs_rejects_deprecated_module_resolution_build_risk() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"moduleResolution":"node10"}}"#,
        )
        .unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("moduleResolution=node10")
        ));
    }

    #[test]
    fn nextjs_rejects_invalid_typescript_exact_version() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"5.0.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("typescript dependency")
        ));
    }

    #[test]
    fn nextjs_rejects_next_react_major_mismatch() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^15.0.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("Next 15")
        ));
    }

    #[test]
    fn repair_manifest_coherence_restores_known_good_dependency_set() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^15.0.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"5.0.0","@types/node":"^18.0.0","@types/react":"^19.0.0","@types/react-dom":"^19.0.0"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();

        assert!(repair_manifest_coherence(dir.path(), "3011").unwrap());
        let package: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("package.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(dependency_version(&package, "next"), Some("^14.2.0"));
        assert_eq!(dependency_version(&package, "react"), Some("^18.3.0"));
        assert_eq!(
            dependency_version(&package, "@types/react"),
            Some("^18.3.0")
        );
        assert_eq!(dependency_version(&package, "typescript"), Some("^5.5.0"));
        assert!(verify(dir.path(), "3011").is_pass());
    }

    #[test]
    fn nextjs_allows_legacy_14_0_dependency_range_until_build_verifier_runs() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"14.0.0","react":"18.2.0","react-dom":"18.2.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        assert!(verify(dir.path(), "3011").is_pass());
    }

    #[test]
    fn nextjs_rejects_interactive_app_page_without_use_client() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#"export default function Page() {
  return <canvas ref={() => {}} onKeyDown={() => {}} />;
}"#,
        )
        .unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("\"use client\"")
        ));

        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";

export default function Page() {
  return <canvas ref={() => {}} onKeyDown={() => {}} />;
}"#,
        )
        .unwrap();
        assert!(verify(dir.path(), "3011").is_pass());
    }

    #[test]
    fn nextjs_rejects_alias_without_baseurl_or_paths() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "import Widget from '@/Widget'; export default function Page(){return <Widget/>;}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"paths":{"@/*":["./src/*"]}}}"#,
        )
        .unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("baseUrl/paths")
        ));
    }

    #[test]
    fn nextjs_rejects_missing_tailwind_toolchain_when_tailwind_used() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("src/app/globals.css"),
            "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n",
        )
        .unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("tailwind_contract_failure") && reason.contains("Tailwind")
        ));
    }

    #[test]
    fn nextjs_accepts_tailwind_cjs_config_variants() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0","tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/layout.tsx"),
            "import './globals.css';\nexport default function Layout({children}:{children:React.ReactNode}){return children;}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/global.d.ts"),
            "declare module \"*.css\";\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/globals.css"),
            "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("tailwind.config.cjs"),
            "module.exports = { content: ['./src/**/*.{ts,tsx}'], theme: { extend: {} }, plugins: [] };\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("postcss.config.cjs"),
            "module.exports = { plugins: { tailwindcss: {}, autoprefixer: {} } };\n",
        )
        .unwrap();
        assert!(verify(dir.path(), "3011").is_pass());
    }

    #[test]
    fn nextjs_rejects_tailwind_without_autoprefixer_dependency() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0","tailwindcss":"^3.4.19","postcss":"^8.5.15"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/layout.tsx"),
            "import './globals.css';\nexport default function Layout({children}:{children:React.ReactNode}){return children;}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/global.d.ts"),
            "declare module \"*.css\";\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/globals.css"),
            "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("tailwind.config.js"),
            "module.exports = { content: ['./src/**/*.{ts,tsx}'], theme: { extend: {} }, plugins: [] };\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("postcss.config.js"),
            "module.exports = { plugins: { tailwindcss: {} } };\n",
        )
        .unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("autoprefixer")
        ));
    }

    #[test]
    fn nextjs_rejects_tailwind_postcss_config_without_plugins() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0","tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/layout.tsx"),
            "import './globals.css';\nexport default function Layout({children}:{children:React.ReactNode}){return children;}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/global.d.ts"),
            "declare module \"*.css\";\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/globals.css"),
            "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("tailwind.config.js"),
            "module.exports = { content: ['./src/**/*.{ts,tsx}'], theme: { extend: {} }, plugins: [] };\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("postcss.config.js"),
            "module.exports = { plugins: {} };\n",
        )
        .unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("PostCSS config must include the Tailwind plugin")
        ));
    }

    #[test]
    fn nextjs_rejects_tailwind_directive_css_not_imported_by_layout() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0","tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/global.d.ts"),
            "declare module \"*.css\";\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/globals.css"),
            "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("tailwind.config.js"),
            "module.exports = { content: ['./src/**/*.{ts,tsx}'], theme: { extend: {} }, plugins: [] };\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("postcss.config.js"),
            "module.exports = { plugins: { tailwindcss: {}, autoprefixer: {} } };\n",
        )
        .unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("@tailwind CSS file must be imported")
        ));
    }

    #[test]
    fn nextjs_allows_plain_css_without_tailwind_toolchain() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("src/app/layout.tsx"),
            "import './globals.css';\nexport default function Layout({children}:{children:React.ReactNode}){return children;}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/global.d.ts"),
            "declare module \"*.css\";\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/globals.css"),
            "body { margin: 0; background: #05070d; color: white; }\n",
        )
        .unwrap();
        assert!(verify(dir.path(), "3011").is_pass());
    }

    #[test]
    fn nextjs_manifest_coherence_adds_tailwind_toolchain_before_install() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("src/app/layout.tsx"),
            "import './globals.css';\nexport default function Layout({children}:{children:React.ReactNode}){return children;}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/global.d.ts"),
            "declare module \"*.css\";\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/globals.css"),
            "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("tailwind.config.js"),
            "module.exports = { content: ['./src/**/*.{ts,tsx}'], theme: { extend: {} }, plugins: [] };\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("postcss.config.js"),
            "module.exports = { plugins: { tailwindcss: {}, autoprefixer: {} } };\n",
        )
        .unwrap();

        assert!(repair_manifest_coherence(dir.path(), "3011").unwrap());
        let package: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("package.json")).unwrap(),
        )
        .unwrap();
        for dep in ["tailwindcss", "postcss", "autoprefixer"] {
            assert!(package_has_dependency(&package, dep), "{dep}");
        }
        assert!(verify(dir.path(), "3011").is_pass());
    }

    fn complete_app() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(dir.path().join("package.json"), package_json()).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page(){return null;}",
        )
        .unwrap();
        std::fs::write(dir.path().join("src/app/layout.tsx"), "export default function Layout({children}:{children:React.ReactNode}){return children;}").unwrap();
        dir
    }
}
