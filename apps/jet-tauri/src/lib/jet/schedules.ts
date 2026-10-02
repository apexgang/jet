import { invoke } from "@tauri-apps/api/core";
import type { PlaneId } from "./planes";
export type Schedule = { id: string; conversationId: string; timeZone: string; localTime: string; prompt: string; nextDueAtUnixMs: string };
export type ScheduleRequest = { conversationId: string; timeZone: string; localTime: string; prompt: string; attempt: string };
export const loadSchedules = (planeId: PlaneId, conversationId: string): Promise<Schedule[]> => invoke("load_schedules", { planeId, conversationId });
export const createSchedule = (planeId: PlaneId, request: ScheduleRequest): Promise<Schedule> => invoke("create_schedule", { planeId, request });
export const cancelSchedule = (planeId: PlaneId, scheduleId: string, attempt: string): Promise<void> => invoke("cancel_schedule", { planeId, scheduleId, attempt });
