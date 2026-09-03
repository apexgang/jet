//! Translation between core domain types and versioned wire types
//! (ADR-0049). This is the only place the two vocabularies meet.

use std::time::{SystemTime, UNIX_EPOCH};

use jet_core::{CoreError, ErrorCategory, PlaneStatus, Query, QueryResult};
use jet_protocol as wire;

pub(crate) fn query(request: &wire::QueryRequest) -> Query {
	match request {
		wire::QueryRequest::Status => Query::Status,
	}
}

pub(crate) fn query_result(result: QueryResult) -> wire::QueryResponse {
	match result {
		QueryResult::Status(status) => {
			wire::QueryResponse::Status(plane_status(&status))
		}
	}
}

fn plane_status(status: &PlaneStatus) -> wire::PlaneStatus {
	wire::PlaneStatus {
		plane_id: status.plane_id.0,
		daemon_starts: status.daemon_starts,
		started_at_unix_ms: unix_ms(status.started_at),
		core_version: status.core_version.into(),
	}
}

pub(crate) fn error(error: CoreError) -> wire::WireError {
	wire::WireError {
		category: category(error.category),
		code: error.code.into(),
		retryable: error.retryable,
		message: error.message,
	}
}

fn category(category: ErrorCategory) -> wire::ErrorCategory {
	match category {
		ErrorCategory::InvalidInput => wire::ErrorCategory::InvalidInput,
		ErrorCategory::Unauthorized => wire::ErrorCategory::Unauthorized,
		ErrorCategory::Conflict => wire::ErrorCategory::Conflict,
		ErrorCategory::Unavailable => wire::ErrorCategory::Unavailable,
		ErrorCategory::Incompatible => wire::ErrorCategory::Incompatible,
		ErrorCategory::RateLimited => wire::ErrorCategory::RateLimited,
		ErrorCategory::NotFound => wire::ErrorCategory::NotFound,
		ErrorCategory::OutcomeUnknown => wire::ErrorCategory::OutcomeUnknown,
		ErrorCategory::Internal => wire::ErrorCategory::Internal,
	}
}

fn unix_ms(time: SystemTime) -> i64 {
	match time.duration_since(UNIX_EPOCH) {
		Ok(elapsed) => i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
		Err(behind) => i64::try_from(behind.duration().as_millis())
			.map_or(i64::MIN, |ms| -ms),
	}
}
