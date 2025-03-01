use serde::{Deserialize, Serialize};
use tracing::{
    span::{Attributes, Id},
    Subscriber,
};
use tracing_subscriber::{
    layer::{Context, Layer},
    registry::LookupSpan,
};

mod types;
use crate::types::header::{Header, SamplingDecision};
use types::{
    ids::{SegmentId, TraceId},
    time::Seconds,
    types::Segment,
};

type Err = Box<dyn std::error::Error + Send + Sync + 'static>;

#[derive(Default)]
pub struct XRay {
    resource_arn: Option<String>,
}

impl XRay {
    pub fn with_resource_arn(self, arn: String) -> XRay {
        XRay {
            resource_arn: Some(arn),
        }
    }
}
#[derive(Default, Debug, Serialize, Deserialize)]
struct SharedData {
    pub(crate) trace_id: TraceId,
    ///  A 64-bit identifier for the segment, unique among segments in the same
    ///  trace, in 16 hexadecimal digits.
    pub(crate) id: SegmentId,
    /// The logical name of the service that handled the request, up to 200
    /// characters. For example, your application's name or domain name. Names
    /// can contain Unicode letters, numbers, and whitespace, and the following
    /// symbols: `_`, `.`,`:`, `/`, `%`, `&, `#`, `=`, `+`, `\`, `-`, `@`
    ///
    /// A segment's name should match the domain name or logical name of the
    /// service that generates the segment. However, this is not enforced. Any
    /// application that has permission to PutTraceSegments can send segments
    /// with any name.
    pub(crate) name: String,
    /// Number that is the time the segment was created, in floating point
    /// seconds in epoch time.
    pub(crate) start_time: Seconds,
    #[serde(flatten)]
    pub(crate) state: State,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum State {
    /// Number that is the time the segment was closed.
    Done { end_time: Seconds },
    ///  boolean, set to true instead of specifying an end_time to record that a
    ///  segment is started, but is not complete. Send an in-progress segment
    ///  when your application receives a request that will take a long time to
    ///  serve, to trace the request receipt. When the response is sent, send
    ///  the complete segment to overwrite the in-progress segment. Only send
    ///  one complete segment, and one or zero in-progress segments, per
    ///  request.
    InProgress { in_progress: bool },
}

impl Default for State {
    fn default() -> Self {
        State::InProgress { in_progress: true }
    }
}

#[test]
fn test_shared_data_representation() -> Result<(), Err> {
    let mut data = SharedData::default();
    dbg!(serde_json::to_string(&data)?);
    data.state = State::Done {
        end_time: Seconds::now(),
    };
    dbg!(serde_json::to_string(&data)?);
    Ok(())
}

impl<S> Layer<S> for XRay
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    fn on_new_span(&self, attrs: &Attributes, id: &Id, ctx: Context<S>) {
        let name = attrs.metadata().name();
        let mut data = Segment::begin(name);

        // in lambda context there should be a facade header that is the root of the execution
        // if it exists use it to set the trace id and parent id
        // the parent id will be overridden later when on_follows_from is called
        if let Some(id) = attrs.metadata().fields().field("x-amzn-trace-id") {
            let header = id
                .to_string()
                .parse::<Header>()
                .expect("Unstable to parse header");
            if header.sampling_decision.eq(&SamplingDecision::NotSampled) {
                return;
            }
            data.trace_id = header.trace_id;
            data.parent_id = header.parent_id;
        }
        data.resource_arn = self.resource_arn.clone();

        let span = ctx
            .span(id)
            .expect("in on_new_span but span does not exist");
        span.extensions_mut().insert(data);
    }

    fn on_follows_from(&self, id: &Id, follows: &Id, ctx: Context<S>) {
        let span = ctx.span(id).expect("Span not found, this is a bug");
        let mut ext = span.extensions_mut();
        let data = ext
            .get_mut::<Segment>()
            .expect("span does not have XRay segment");

        let follows_span = ctx
            .span(follows)
            .expect("Span to follow not found, this is a bug");
        let follows_ext = follows_span.extensions();
        let follows_data = follows_ext
            .get::<Segment>()
            .expect("span does not have XRay segment");

        data.parent_id = Some(follows_data.id.clone());
    }

    fn on_close(&self, id: Id, ctx: Context<S>) {
        let span = ctx.span(&id).expect("in on_close but span does not exist");
        let mut ext = span.extensions_mut();
        let data = ext
            .get_mut::<Segment>()
            .expect("span does not have XRay segment");
        data.end();
    }
}
