// The Craft corpus, read through the models a Craft-aware client would use.
import { check } from "./interpreter.ts";
import type { CraftHello, CraftCommand, CraftEvent, ProtocolOffer, CraftExtensionRequest, CraftExtensionReply } from "../CraftModels.ts";

check<CraftHello | CraftCommand | CraftEvent | ProtocolOffer | CraftExtensionRequest | CraftExtensionReply>(
  new URL("../craft-v1.schema.json", import.meta.url),
  new URL("../craft-fixtures.json", import.meta.url),
  "Craft",
);
