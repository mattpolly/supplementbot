use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::events::{EventEnvelope, PipelineEvent};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// EventSink trait — the abstraction all consumers program against
// ---------------------------------------------------------------------------

pub trait EventSink: Send + Sync {
    /// Emit a single event. The sink wraps it in an envelope with the given correlation ID.
    fn emit(&self, correlation_id: Uuid, event: PipelineEvent);

    /// Flush any buffered events to their backing store.
    fn flush(&self) -> std::io::Result<()>;
}

// ---------------------------------------------------------------------------
// JsonlFileSink — appends one JSON object per line to a file
// ---------------------------------------------------------------------------

pub struct JsonlFileSink {
    writer: Mutex<BufWriter<File>>,
    path: PathBuf,
}

impl JsonlFileSink {
    /// Open (or create) a JSONL file for appending events.
    pub fn new(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        Ok(Self {
            writer: Mutex::new(BufWriter::new(file)),
            path,
        })
    }

    /// Read all events back from the file. Useful for evaluation and replay.
    pub fn read_all(path: impl AsRef<Path>) -> std::io::Result<Vec<EventEnvelope>> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut events = Vec::new();
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let envelope: EventEnvelope = serde_json::from_str(&line)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            events.push(envelope);
        }
        Ok(events)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl EventSink for JsonlFileSink {
    fn emit(&self, correlation_id: Uuid, event: PipelineEvent) {
        let envelope = EventEnvelope::new(correlation_id, event);
        if let Ok(json) = serde_json::to_string(&envelope) {
            if let Ok(mut writer) = self.writer.lock() {
                let _ = writeln!(writer, "{}", json);
            }
        }
    }

    fn flush(&self) -> std::io::Result<()> {
        let mut writer = self.writer.lock().map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::Other, "lock poisoned")
        })?;
        writer.flush()
    }
}

// ---------------------------------------------------------------------------
// MemorySink — captures events in memory for tests and evaluation
// ---------------------------------------------------------------------------

pub struct MemorySink {
    events: Mutex<Vec<EventEnvelope>>,
}

impl MemorySink {
    pub fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }

    /// Get a snapshot of all captured events
    pub fn events(&self) -> Vec<EventEnvelope> {
        self.events.lock().unwrap().clone()
    }

    /// How many events have been captured
    pub fn len(&self) -> usize {
        self.events.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Filter events by correlation ID
    pub fn events_for(&self, correlation_id: Uuid) -> Vec<EventEnvelope> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.correlation_id == correlation_id)
            .cloned()
            .collect()
    }
}

impl Default for MemorySink {
    fn default() -> Self {
        Self::new()
    }
}

impl EventSink for MemorySink {
    fn emit(&self, correlation_id: Uuid, event: PipelineEvent) {
        let envelope = EventEnvelope::new(correlation_id, event);
        self.events.lock().unwrap().push(envelope);
    }

    fn flush(&self) -> std::io::Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::CurriculumStage;
    use std::fs;
    use uuid::Uuid;

    fn sample_request_event() -> PipelineEvent {
        PipelineEvent::LlmRequest {
            provider: "anthropic".into(),
            model: "claude-sonnet-4-20250514".into(),
            prompt: "What physiological systems does Magnesium act on?".into(),
            nutraceutical: "Magnesium".into(),
            stage: CurriculumStage::Foundational,
            question_type: "systems".into(),
        }
    }

    fn sample_response_event() -> PipelineEvent {
        PipelineEvent::LlmResponse {
            provider: "anthropic".into(),
            model: "claude-sonnet-4-20250514".into(),
            raw_response: "Magnesium acts on the nervous, muscular, and cardiovascular systems."
                .into(),
            latency_ms: 1234,
            tokens_used: Some(crate::events::TokenUsage {
                input_tokens: 42,
                output_tokens: 18,
            }),
        }
    }

    #[test]
    fn test_memory_sink_capture() {
        let sink = MemorySink::new();
        let corr_id = Uuid::new_v4();

        sink.emit(corr_id, sample_request_event());
        sink.emit(corr_id, sample_response_event());

        assert_eq!(sink.len(), 2);

        let events = sink.events_for(corr_id);
        assert_eq!(events.len(), 2);

        // First event should be the request
        match &events[0].event {
            PipelineEvent::LlmRequest { nutraceutical, .. } => {
                assert_eq!(nutraceutical, "Magnesium");
            }
            other => panic!("expected LlmRequest, got {:?}", other),
        }
    }

    #[test]
    fn test_memory_sink_filters_by_correlation_id() {
        let sink = MemorySink::new();
        let corr_a = Uuid::new_v4();
        let corr_b = Uuid::new_v4();

        sink.emit(corr_a, sample_request_event());
        sink.emit(corr_b, sample_response_event());

        assert_eq!(sink.events_for(corr_a).len(), 1);
        assert_eq!(sink.events_for(corr_b).len(), 1);
    }

    #[test]
    fn test_jsonl_file_roundtrip() {
        let dir = std::env::temp_dir().join("supplementbot_test");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test_events.jsonl");

        // Clean up from previous runs
        let _ = fs::remove_file(&path);

        let corr_id = Uuid::new_v4();

        // Write events
        {
            let sink = JsonlFileSink::new(&path).unwrap();
            sink.emit(corr_id, sample_request_event());
            sink.emit(corr_id, sample_response_event());
            sink.flush().unwrap();
        }

        // Read them back
        let events = JsonlFileSink::read_all(&path).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].correlation_id, corr_id);
        assert_eq!(events[1].correlation_id, corr_id);

        // Verify event content survived
        match &events[0].event {
            PipelineEvent::LlmRequest {
                provider,
                nutraceutical,
                ..
            } => {
                assert_eq!(provider, "anthropic");
                assert_eq!(nutraceutical, "Magnesium");
            }
            other => panic!("expected LlmRequest, got {:?}", other),
        }

        match &events[1].event {
            PipelineEvent::LlmResponse { latency_ms, .. } => {
                assert_eq!(*latency_ms, 1234);
            }
            other => panic!("expected LlmResponse, got {:?}", other),
        }

        // Clean up
        let _ = fs::remove_file(&path);
    }
}
