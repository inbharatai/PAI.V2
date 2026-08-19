use inbharat_harness_core::RootedFs;
use inbharat_harness_core::error::{ErrorCode, Failure, FailureClass, HarnessResult};
use std::path::{Component, Path};

pub(crate) fn create(output: &str, title: &str, force: bool) -> HarnessResult<()> {
    let root = std::env::current_dir().map_err(|error| io_failure("demo.cwd", error))?;
    let rooted = RootedFs::new(&root)?;
    let relative = Path::new(output);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(Failure::invalid(
            "demo-website",
            "output must be a relative path inside the current directory",
        ));
    }
    let absolute = root.join(relative);
    if absolute.exists() && !force {
        return Err(Failure::new(
            ErrorCode::Conflict,
            FailureClass::User,
            "demo-website",
            "output exists; pass --force to replace demo files",
        ));
    }
    rooted.create_dir_all(relative)?;
    let index_path = relative.join("index.html");
    let css_path = relative.join("style.css");
    rooted.write_text_atomic(
        &index_path,
        &format!(
            "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{}</title><link rel=\"stylesheet\" href=\"style.css\"></head><body><main><p class=\"eyebrow\">LOCAL RELEASE CANDIDATE</p><h1>{}</h1><p>Deterministic routing. Bounded tools. Replayable sessions.</p><dl><div><dt>L0</dt><dd>Direct</dd></div><div><dt>L1</dt><dd>Single action</dd></div><div><dt>L2</dt><dd>Finite agent</dd></div><div><dt>L3</dt><dd>Goal workspace</dd></div></dl></main></body></html>\n",
            html_escape(title),
            html_escape(title)
        ),
    )?;
    rooted.write_text_atomic(
        &css_path,
        ":root{color-scheme:dark;font-family:ui-sans-serif,system-ui;background:#0b1020;color:#f3f6ff}body{margin:0;min-height:100vh;display:grid;place-items:center}main{width:min(760px,86vw);padding:4rem;border:1px solid #31405f;border-radius:24px;background:#111a2d;box-shadow:0 32px 90px #0008}.eyebrow{letter-spacing:.18em;color:#72d6b3;font-weight:700}h1{font-size:clamp(2.5rem,8vw,5.5rem);line-height:.95;margin:.4em 0}p{font-size:1.2rem;color:#b9c6df}dl{display:grid;grid-template-columns:repeat(4,1fr);gap:1rem;margin-top:3rem}dl div{padding:1rem;border-top:2px solid #72d6b3}dt{font-size:1.5rem;font-weight:800}dd{margin:.35rem 0 0;color:#b9c6df}@media(max-width:600px){main{padding:2rem}dl{grid-template-columns:1fr 1fr}}\n",
    )?;
    println!(
        "created {} and {}",
        index_path.display(),
        css_path.display()
    );
    Ok(())
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn io_failure(operation: &str, error: std::io::Error) -> Failure {
    Failure::new(
        ErrorCode::ToolFailed,
        FailureClass::Execution,
        operation,
        "local I/O failed",
    )
    .with_detail("io_kind", format!("{:?}", error.kind()))
}
