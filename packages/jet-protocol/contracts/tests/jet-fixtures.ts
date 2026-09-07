// The client corpus, read through the models the Tauri GUI compiles against.
import { check } from "./interpreter.ts";
import type {
  ClientHello,
  ClientMessage,
  ServerHello,
  ServerMessage,
} from "../../../../apps/jet-tauri/src/lib/protocol/JetModels.ts";

check<ClientHello | ServerHello | ClientMessage | ServerMessage>(
  new URL("../jet-v1.schema.json", import.meta.url),
  new URL("../jet-fixtures.json", import.meta.url),
  "Jet",
);
