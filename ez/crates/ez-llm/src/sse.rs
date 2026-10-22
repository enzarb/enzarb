//! Minimal server-sent-events decoder over a byte stream.

use futures::{Stream, StreamExt};

#[derive(Debug, Default, Clone, PartialEq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Default)]
pub struct SseDecoder {
    buf: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

impl SseDecoder {
    /// Feed a chunk of bytes, returning any complete events.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let raw: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&raw);
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if !self.data.is_empty() || self.event.is_some() {
                    out.push(SseEvent {
                        event: self.event.take(),
                        data: std::mem::take(&mut self.data).join("\n"),
                    });
                }
            } else if let Some(v) = line.strip_prefix("data:") {
                self.data.push(v.strip_prefix(' ').unwrap_or(v).to_string());
            } else if let Some(v) = line.strip_prefix("event:") {
                self.event = Some(v.trim().to_string());
            }
        }
        out
    }

    /// Flush at end of stream: a trailing line without newline, then any pending event.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut out = if self.buf.is_empty() {
            Vec::new()
        } else {
            self.push(b"\n")
        };
        out.extend(self.push(b"\n"));
        out
    }
}

pub fn events(
    resp: reqwest::Response,
) -> impl Stream<Item = anyhow::Result<SseEvent>> + Send + Unpin {
    decode(resp.bytes_stream())
}

/// Decode a byte stream into events, flushing a final unterminated event at EOF.
pub fn decode<S, B, E>(bytes: S) -> impl Stream<Item = anyhow::Result<SseEvent>> + Send + Unpin
where
    S: Stream<Item = Result<B, E>> + Send + Unpin + 'static,
    B: AsRef<[u8]>,
    E: Into<anyhow::Error>,
{
    let state = (bytes, SseDecoder::default(), false);
    Box::pin(
        futures::stream::unfold(state, |(mut bytes, mut decoder, done)| async move {
            if done {
                return None;
            }
            let (batch, done) = match bytes.next().await {
                Some(Ok(chunk)) => (Ok(decoder.push(chunk.as_ref())), false),
                Some(Err(e)) => (Err(e.into()), true),
                None => (Ok(decoder.finish()), true),
            };
            Some((batch, (bytes, decoder, done)))
        })
        .flat_map(|r| {
            futures::stream::iter(match r {
                Ok(v) => v.into_iter().map(Ok).collect::<Vec<_>>(),
                Err(e) => vec![Err(e)],
            })
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_split_events() {
        let mut d = SseDecoder::default();
        assert!(d.push(b"event: ping\nda").is_empty());
        let ev = d.push(b"ta: {\"a\":1}\n\ndata: x\n\n");
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[0].event.as_deref(), Some("ping"));
        assert_eq!(ev[0].data, "{\"a\":1}");
        assert_eq!(ev[1].data, "x");
    }

    #[test]
    fn finish_flushes_unterminated_event() {
        let mut d = SseDecoder::default();
        assert!(d.push(b"data: [DONE]").is_empty());
        let ev = d.finish();
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].data, "[DONE]");
        assert!(d.finish().is_empty());
    }

    #[tokio::test]
    async fn decode_stream_flushes_at_eof() {
        let chunks: Vec<Result<&[u8], std::io::Error>> =
            vec![Ok(b"data: a\n\nda"), Ok(b"ta: b\r\n")];
        let got: Vec<String> = decode(futures::stream::iter(chunks))
            .map(|e| e.unwrap().data)
            .collect()
            .await;
        assert_eq!(got, ["a", "b"]);
    }
}
