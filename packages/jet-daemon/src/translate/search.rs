//! Search hits, from the core's form to the wire's (ADR-0036).

use jet_core::{SearchField, SearchHit, SearchResult};
use jet_protocol as wire;

pub(super) fn result(result: SearchResult) -> wire::SearchResult {
	wire::SearchResult {
		cursor: result.cursor.0,
		indexed_through: result.indexed_through.0,
		hits: result.hits.into_iter().map(hit).collect(),
	}
}

fn hit(hit: SearchHit) -> wire::SearchHit {
	wire::SearchHit {
		conversation_id: hit.conversation_id.0,
		sequence: hit.sequence.0,
		field: match hit.field {
			SearchField::Path => wire::SearchField::Path,
			SearchField::Branch => wire::SearchField::Branch,
		},
		excerpt: hit.excerpt,
	}
}
