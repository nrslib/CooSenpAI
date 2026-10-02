use habitua_connectome::{RateActivitySamples, RateGraph};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize)]
struct PositionSample {
    graph_fingerprint: String,
    total_neurons: usize,
    positioned_neurons: usize,
    neurons: Vec<Soma>,
}
#[derive(Debug, Deserialize)]
struct Soma {
    body_id: String,
    position: [f64; 3],
}

pub(super) struct ActivityDisplay {
    positions: Option<PositionSample>,
    pub(super) samples: RateActivitySamples,
    latest: Option<(String, Value)>,
}
impl ActivityDisplay {
    pub(super) fn for_graph(graph: &RateGraph) -> Self {
        let source: PositionSample =
            serde_json::from_str(include_str!("../../assets/malecns-soma-sample.json"))
                .expect("bundled soma asset");
        let matches = source.graph_fingerprint == graph.fingerprint()
            && source.total_neurons == graph.neuron_count();
        let indices = if matches {
            source
                .neurons
                .iter()
                .map(|neuron| {
                    graph
                        .root_ids
                        .binary_search(&neuron.body_id.parse::<u64>().expect("bundled body ID"))
                        .expect("bundled body ID in graph")
                })
                .collect()
        } else {
            Vec::new()
        };
        Self {
            positions: matches.then_some(source),
            samples: RateActivitySamples {
                neuron_indices: indices,
                frames: Vec::new(),
            },
            latest: None,
        }
    }
    pub(super) fn clear(&mut self) {
        self.latest = None;
    }
    pub(super) fn capture(&mut self, image_sha256: &str, total_steps: usize, hmax: f64) {
        let Some(positions) = &self.positions else {
            return;
        };
        let frames: Vec<Value> = self.samples.frames.iter().map(|(step, values)| json!({"step": step, "values": values.iter().flat_map(|value| value.to_le_bytes()).map(|byte| format!("{byte:02x}")).collect::<String>()})).collect();
        let neurons: Vec<Value> = positions
            .neurons
            .iter()
            .map(|neuron| json!({"body_id": neuron.body_id, "position": neuron.position}))
            .collect();
        self.latest = Some((
            image_sha256.to_owned(),
            json!({
                "schema": "rate-activity-v1", "graph_fingerprint": positions.graph_fingerprint,
                "image_sha256": image_sha256, "total_neurons": positions.total_neurons,
                "positioned_neurons": positions.positioned_neurons, "total_steps": total_steps,
                "hmax": hmax, "encoding": "f32-le-hex", "coordinate_source": "MaleCNS v1.0 somaLocation",
                "neurons": neurons, "frames": frames
            }),
        ));
        self.samples.frames.clear();
    }
    pub(super) fn response(
        &self,
        input_id: &str,
        image_sha256: &str,
        cached: bool,
        replayed: bool,
    ) -> Value {
        if !cached
            && !replayed
            && let Some((digest, value)) = &self.latest
            && digest == image_sha256
        {
            let mut value = value.clone();
            value["input_id"] = json!(input_id);
            return value;
        }
        json!({"schema": "rate-activity-v1", "unavailable_reason": if self.positions.is_none() { "positions-unavailable" } else if replayed { "observation-replayed" } else { "image-cache-hit" }})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_positions_and_serialized_activity_fit_the_protocol_budget() {
        let positions: PositionSample =
            serde_json::from_str(include_str!("../../assets/malecns-soma-sample.json")).unwrap();
        assert_eq!(positions.neurons.len(), 1024);
        assert_eq!(positions.total_neurons, 211_577);
        assert_eq!(positions.positioned_neurons, 141_781);
        assert!(
            positions
                .neurons
                .windows(2)
                .all(|pair| pair[0].body_id.parse::<u64>().unwrap()
                    < pair[1].body_id.parse::<u64>().unwrap())
        );
        assert!(
            positions
                .neurons
                .iter()
                .all(|neuron| neuron.position.iter().all(|value| value.is_finite()))
        );
        let mut display = ActivityDisplay {
            positions: Some(positions),
            samples: RateActivitySamples {
                neuron_indices: (0..1024).collect(),
                frames: (0..=16)
                    .map(|frame| (frame * 2, vec![1e-6; 1024]))
                    .collect(),
            },
            latest: None,
        };
        let digest = "a".repeat(64);
        display.capture(&digest, 32, 10.0);
        let response = display.response("input-1", &digest, false, false);
        let encoded = serde_json::to_vec(&response).unwrap();
        assert!(encoded.len() < 224 * 1024);
        assert_eq!(
            response["frames"][1]["values"].as_str().unwrap().get(..8),
            Some("bd378635")
        );
        assert_eq!(
            display.response("input-2", &digest, true, false)["unavailable_reason"],
            "image-cache-hit"
        );
        assert_eq!(
            display.response("input-1", &digest, false, true)["unavailable_reason"],
            "observation-replayed"
        );
        display.clear();
        assert!(
            display
                .response("input-3", &digest, true, false)
                .get("frames")
                .is_none()
        );
        display.positions = None;
        assert_eq!(
            display.response("input-4", &digest, false, false)["unavailable_reason"],
            "positions-unavailable"
        );
    }
}
