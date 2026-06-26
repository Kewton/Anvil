use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::planner::profile::ProfileQualityExpectations;
use crate::planner::profile::profile_failure;
use crate::planner::verify::{VerificationReport, VerifyStatus};

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
         package.json must include next, react, react-dom and scripts.build = `next build`. \
         For TypeScript/TSX apps, create tsconfig.json before treating the app as complete. \
         Do not use deprecated moduleResolution=node10; use bundler or node16, \
         or set ignoreDeprecations to 6.0 when needed.{port}"
    )
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
    deps.insert("next".to_string(), Value::String("^14.2.0".to_string()));
    deps.insert("react".to_string(), Value::String("^18.3.0".to_string()));
    deps.insert(
        "react-dom".to_string(),
        Value::String("^18.3.0".to_string()),
    );
    let dev_deps = object_entry(&mut package, "devDependencies");
    dev_deps.insert(
        "typescript".to_string(),
        Value::String("^5.5.0".to_string()),
    );
    dev_deps.insert(
        "@types/node".to_string(),
        Value::String("^20.14.0".to_string()),
    );
    dev_deps.insert(
        "@types/react".to_string(),
        Value::String("^18.3.0".to_string()),
    );
    dev_deps.insert(
        "@types/react-dom".to_string(),
        Value::String("^18.3.0".to_string()),
    );
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
    let uses_tailwind = package_has_dependency(package, "tailwindcss")
        || contains_in_files(root, "@tailwind")
        || root.join("tailwind.config.js").is_file()
        || root.join("tailwind.config.ts").is_file();
    if !uses_tailwind {
        return None;
    }
    for dep in ["tailwindcss", "postcss"] {
        if !package_has_dependency(package, dep) {
            return Some(format!("Tailwind toolchain dependency missing: {dep}"));
        }
    }
    if !(root.join("tailwind.config.js").is_file() || root.join("tailwind.config.ts").is_file()) {
        return Some("Tailwind config file missing".to_string());
    }
    if !(root.join("postcss.config.js").is_file()
        || root.join("postcss.config.mjs").is_file()
        || root.join("postcss.config.cjs").is_file())
    {
        return Some("PostCSS config file missing for Tailwind".to_string());
    }
    let layout = std::fs::read_to_string(root.join("src/app/layout.tsx"))
        .or_else(|_| std::fs::read_to_string(root.join("app/layout.tsx")))
        .unwrap_or_default();
    if !layout.contains("globals.css") {
        return Some("globals.css must be imported by app layout".to_string());
    }
    None
}

fn package_has_dependency(package: &Value, name: &str) -> bool {
    ["dependencies", "devDependencies"]
        .iter()
        .filter_map(|key| package.get(*key).and_then(Value::as_object))
        .any(|deps| deps.contains_key(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::verify::VerifyStatus;

    fn package_json() -> &'static str {
        r#"{"dependencies":{"next":"x","react":"x","react-dom":"x"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#
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
            r#"{"dependencies":{"next":"x","react":"x","react-dom":"x"},"scripts":{"build":"echo ok","dev":"next dev -p 3011"}}"#,
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
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("Tailwind")
        ));
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
