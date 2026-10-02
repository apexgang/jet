//! Narrow daily-schedule adapter; jetd owns timing, delivery and validation.
use super::{errors::PublicError, JetBridge};
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScheduleView {
    id: String,
    conversation_id: String,
    time_zone: String,
    local_time: String,
    prompt: String,
    next_due_at_unix_ms: String,
}
impl From<jet_protocol::ScheduledTask> for ScheduleView {
    fn from(task: jet_protocol::ScheduledTask) -> Self {
        Self {
            id: task.schedule_id.to_string(),
            conversation_id: task.conversation_id.to_string(),
            time_zone: task.time_zone,
            local_time: task.local_time,
            prompt: task.prompt,
            next_due_at_unix_ms: task.next.due_at_unix_ms.to_string(),
        }
    }
}

fn id(value: &str) -> Result<Uuid, PublicError> {
    Uuid::parse_str(value).map_err(|_| {
        PublicError::invalid_input(
            "schedule.identifier_invalid",
            "Reload the task before changing its schedule.",
        )
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateSchedule {
    conversation_id: String,
    time_zone: String,
    local_time: String,
    prompt: String,
    attempt: String,
}

#[tauri::command]
pub(crate) async fn load_schedules(
    bridge: State<'_, JetBridge>,
    plane_id: Option<String>,
    conversation_id: String,
) -> Result<Vec<ScheduleView>, PublicError> {
    let conversation = id(&conversation_id)?;
    let (binding, plane) = bridge.plane(plane_id.as_deref())?;
    async {
        let client = plane
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        let snapshot = client
            .scheduled_tasks(conversation)
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        if snapshot
            .tasks
            .iter()
            .any(|task| task.conversation_id != conversation)
        {
            return Err(PublicError::internal());
        }
        Ok(snapshot.tasks.into_iter().map(ScheduleView::from).collect())
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

#[tauri::command]
pub(crate) async fn create_schedule(
    bridge: State<'_, JetBridge>,
    plane_id: Option<String>,
    request: CreateSchedule,
) -> Result<ScheduleView, PublicError> {
    let conversation = id(&request.conversation_id)?;
    let attempt = id(&request.attempt)?;
    if request.prompt.trim().is_empty()
        || request.prompt.len() > 8192
        || request.time_zone.len() > 128
        || request.local_time.len() != 8
    {
        return Err(PublicError::invalid_input(
            "schedule.input_invalid",
            "Enter a daily time, a time zone, and instructions up to 8,192 UTF-8 bytes.",
        ));
    }
    let (binding, plane) = bridge.plane(plane_id.as_deref())?;
    async {
        let client = plane
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        let task = client
            .create_schedule(
                attempt,
                conversation,
                request.time_zone,
                request.local_time,
                request.prompt,
            )
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        Ok(task.into())
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

#[tauri::command]
pub(crate) async fn cancel_schedule(
    bridge: State<'_, JetBridge>,
    plane_id: Option<String>,
    schedule_id: String,
    attempt: String,
) -> Result<(), PublicError> {
    let schedule = id(&schedule_id)?;
    let attempt = id(&attempt)?;
    let (binding, plane) = bridge.plane(plane_id.as_deref())?;
    async {
        let client = plane
            .connect()
            .await
            .map_err(|error| PublicError::from_client(&error))?;
        client
            .cancel_schedule(attempt, schedule)
            .await
            .map_err(|error| PublicError::from_client(&error))
    }
    .await
    .map_err(|error| bridge.settle(&binding, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jet::fake_plane::{answer, complete, exchange, local_plane, next};
    use jet_protocol::{
        ClientMessage, CommandRequest, CommandResponse, QueryRequest, QueryResponse,
        ScheduleFiring, ScheduledTask, ScheduledTasks,
    };

    #[tokio::test]
    async fn daily_schedule_round_trip_used_the_existing_wire_contract() {
        let fake = local_plane();
        let conversation_id = Uuid::from_u128(10);
        let command_id = Uuid::from_u128(11);
        let task = ScheduledTask {
            schedule_id: Uuid::from_u128(12),
            conversation_id,
            authorized_by: Uuid::from_u128(13),
            time_zone: "Europe/Moscow".into(),
            local_time: "09:00:00".into(),
            prompt: "Review recent changes".into(),
            next: ScheduleFiring {
                firing_id: Uuid::from_u128(14),
                intended_local: "2026-09-27T09:00:00".into(),
                due_at_unix_ms: 1_790_488_800_000,
            },
        };
        let request = async {
            let client = fake.bridge().local().connect().await.unwrap();
            let created = client
                .create_schedule(
                    command_id,
                    conversation_id,
                    task.time_zone.clone(),
                    task.local_time.clone(),
                    task.prompt.clone(),
                )
                .await
                .unwrap();
            let snapshot = client.scheduled_tasks(conversation_id).await.unwrap();
            client
                .cancel_schedule(Uuid::from_u128(15), task.schedule_id)
                .await
                .unwrap();
            (created, snapshot)
        };
        let serve = async {
            let (mut reader, mut writer) = fake.accept().await;
            let (stream, message) = next(&mut reader).await;
            match &message {
                ClientMessage::Command {
                    command_id: actual,
                    command,
                    ..
                } => {
                    assert_eq!(*actual, command_id);
                    assert_eq!(
                        *command,
                        CommandRequest::CreateSchedule {
                            conversation_id,
                            time_zone: task.time_zone.clone(),
                            local_time: task.local_time.clone(),
                            prompt: task.prompt.clone()
                        }
                    );
                }
                other => panic!("expected create, got {other:?}"),
            }
            complete(
                &mut writer,
                stream,
                &message,
                CommandResponse::ScheduleCreated { task: task.clone() },
            )
            .await;
            let (stream, message) = next(&mut reader).await;
            assert!(
                matches!(&message, ClientMessage::Query { query: QueryRequest::ScheduledTasks { conversation_id: id }, .. } if *id == conversation_id)
            );
            answer(
                &mut writer,
                stream,
                &message,
                QueryResponse::ScheduledTasks(ScheduledTasks {
                    cursor: 9007199254740993,
                    tasks: vec![task.clone()],
                }),
            )
            .await;
            let (stream, message) = next(&mut reader).await;
            assert!(
                matches!(&message, ClientMessage::Command { command: CommandRequest::CancelSchedule { schedule_id }, .. } if *schedule_id == task.schedule_id)
            );
            complete(
                &mut writer,
                stream,
                &message,
                CommandResponse::ScheduleCanceled {
                    schedule_id: task.schedule_id,
                },
            )
            .await;
        };
        let ((created, snapshot), _) = exchange(request, serve).await;
        assert_eq!(created, task);
        assert_eq!(
            snapshot,
            ScheduledTasks {
                cursor: 9007199254740993,
                tasks: vec![task]
            }
        );
    }
}
