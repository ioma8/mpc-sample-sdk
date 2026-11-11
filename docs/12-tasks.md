# Tasks and render jobs

Rendering can take seconds. Do it on a worker; keep the UI responsive. The SDK
gives you a generic one-shot `Task<T>` and a `RenderJob`/`RenderOutput` pair for
audio work.

## The shape

```rust
let mut task = Task::spawn(
    key,                 // your screen / job id
    value,               // moved to the worker
    move |value| {       // runs on the worker
        // ... produce the result ...
        Ok(())
    },
);
```

`Task::take()` never blocks:

```rust
match task.take() {
    Err(TaskError::Running) => { /* not done yet */ }
    Err(TaskError::Render(code)) => { /* feature-defined failure */ }
    Err(TaskError::Panicked) => { /* worker panicked */ }
    Err(TaskError::AlreadyTaken) => { /* result was already consumed */ }
    Ok(value) => { /* use it */ }
}
```

## A complete render example

```rust
use mpc_sample_sdk::render::{RenderJob, RenderOutput};
use mpc_sample_sdk::task::{Task, TaskError};
use mpc_sample_sdk::{Mod, PaintEvent};

#[derive(Default)]
struct Renderer {
    task: Option<Task<RenderJob>>,
    status: &'static str,
}

impl Renderer {
    fn start(&mut self) {
        let screen = mpc_sample_sdk::platform::page::screen();
        // SAFETY: `screen` is the live Sample page recorded by the paint hook.
        let Some(selected) = unsafe { mpc_sample_sdk::platform::selection::current(screen) } else {
            self.status = "no sample";
            return;
        };
        let Ok(input) = selected.read_audio() else {
            self.status = "cannot read audio";
            return;
        };
        let job = RenderJob::new(selected, 7, input);
        let task = Task::spawn(screen, job, move |job: &mut RenderJob| {
            // Replace this with your DSP.
            let channels = u32::from(job.source.channels());
            let output = RenderOutput::new(job.input.clone(), channels).ok_or(1)?;
            job.push_output(output);
            mpc_sample_sdk::draw::repaint_painted();
            Ok(())
        });
        self.task = Some(task);
        self.status = "rendering";
        mpc_sample_sdk::draw::repaint_painted();
    }

    fn finish(&mut self) {
        let outcome = self.task.as_mut().map(Task::take);
        match outcome {
            Some(Err(TaskError::Running)) => {}
            Some(Err(TaskError::Render(code))) => {
                self.task = None;
                self.status = "render failed";
                mpc_sample_sdk::platform::status::set_status(&format!("error {code}"));
            }
            Some(Err(TaskError::Panicked)) => {
                self.task = None;
                self.status = "worker panicked";
            }
            Some(Err(TaskError::AlreadyTaken)) | None => {
                self.task = None;
                self.status = "invalid task";
            }
            Some(Ok(job)) => {
                self.task = None;
                let frames = job.outputs.first().map_or(0, |output| output.frames);
                self.status = "done";
                mpc_sample_sdk::platform::status::set_status(&format!("{frames} frames"));
            }
        }
        mpc_sample_sdk::draw::repaint_painted();
    }
}

impl Mod for Renderer {
    fn on_paint(&mut self, _graphics: &mut mpc_sample_sdk::draw::Graphics, event: PaintEvent) {
        if event.sample_page {
            self.finish();
        }
    }
}

mpc_sample_sdk::export_mod!(Renderer);
```

## Things to remember

- `Task` owns the value. You cannot touch it on the UI thread while it runs.
- `take()` is non-blocking and returns the value **once**.
- The worker must call `draw::repaint_painted()` when it finishes; otherwise an
  idle UI may never call `finish`.
- The release profile uses `panic = "abort"`. `TaskError::Panicked` is only
  observable on a build that unwinds; treat panics as a bug, not a control path.
- The `key` is usually the screen the job belongs to. Use `task.key()` to check
  that the user is still on that screen before accepting the result.
