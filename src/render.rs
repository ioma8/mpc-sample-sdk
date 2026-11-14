//! One audio render request and its outputs.
//!
//! A [`RenderJob`] is the immutable identity of the selected sample plus the
//! input audio and the output buffers a backend fills.  The worker and result
//! handoff live in [`crate::task`]; this module deliberately knows nothing about
//! HPSS, effects or the synth.

use crate::platform::selection::Selection;

/// One rendered buffer.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderOutput {
    /// Interleaved f32 audio.
    pub audio: Vec<f32>,
    /// Length in frames.
    pub frames: usize,
    /// Interleaved channel count.
    pub channels: u32,
}

impl RenderOutput {
    /// A buffer whose frame count follows from its channel count.
    ///
    /// Returns `None` when `channels` is zero, the audio is empty, or the length
    /// is not a whole number of frames.
    #[must_use]
    pub fn new(audio: Vec<f32>, channels: u32) -> Option<Self> {
        let channels_usize = usize::try_from(channels).ok().filter(|count| *count > 0)?;
        let frames = audio.len() / channels_usize;
        if frames == 0 || frames.checked_mul(channels_usize) != Some(audio.len()) {
            return None;
        }
        Some(Self {
            audio,
            frames,
            channels,
        })
    }

    /// A buffer with an explicit frame count.
    ///
    /// Returns `None` when the geometry does not describe the audio exactly.
    #[must_use]
    pub fn with_frames(audio: Vec<f32>, frames: usize, channels: u32) -> Option<Self> {
        let channels_usize = usize::try_from(channels).ok().filter(|count| *count > 0)?;
        if frames == 0 || frames.checked_mul(channels_usize) != Some(audio.len()) {
            return None;
        }
        Some(Self {
            audio,
            frames,
            channels,
        })
    }
}

/// One audio render request.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderJob {
    /// The exact selection the job was started from.
    pub source: Selection,
    /// The feature's action code; the SDK does not interpret it.
    pub kind: i32,
    /// Interleaved input audio; empty for actions that synthesize or capture.
    pub input: Vec<f32>,
    /// The outputs the backend filled, in the backend's order.
    pub outputs: Vec<RenderOutput>,
}

impl RenderJob {
    /// A job with the selected source, action code and input audio.
    #[must_use]
    pub fn new(source: Selection, kind: i32, input: Vec<f32>) -> Self {
        Self {
            source,
            kind,
            input,
            outputs: Vec::new(),
        }
    }

    /// Add one output buffer.
    pub fn push_output(&mut self, output: RenderOutput) {
        self.outputs.push(output);
    }
}

#[cfg(test)]
mod tests {
    use super::{RenderJob, RenderOutput};
    use crate::platform::selection::Selection;

    fn source() -> Selection {
        Selection::fixture(1, 2, 3, 4, 2, 44_100)
    }

    #[test]
    fn an_output_derives_its_frame_count() {
        let output = RenderOutput::new(vec![0.0; 8], 2).expect("stereo");
        assert_eq!(output.frames, 4);
        assert!(RenderOutput::new(vec![0.0; 3], 2).is_none());
        assert!(RenderOutput::new(Vec::new(), 2).is_none());
    }

    #[test]
    fn an_explicit_frame_count_is_checked() {
        assert!(RenderOutput::with_frames(vec![0.0; 8], 4, 2).is_some());
        assert!(RenderOutput::with_frames(vec![0.0; 8], 3, 2).is_none());
    }

    #[test]
    fn a_job_starts_with_no_outputs() {
        let job = RenderJob::new(source(), 0, vec![0.0; 8]);
        assert_eq!(job.source, source());
        assert!(job.outputs.is_empty());
    }
}
