import { pathToFileURL } from "node:url";
import type { Input } from "@openai/codex-sdk";
import type { ContentBlockParam } from "@anthropic-ai/sdk/resources/messages/messages.js";
import type { FilePartInput, TextPartInput } from "@opencode-ai/sdk/v2";
import { imageMime } from "../images.js";
import type { ProviderImageAttachment } from "../types.js";

export function codexInput(
  message: string,
  images: readonly ProviderImageAttachment[],
): Input {
  if (images.length === 0) return message;
  return [
    { type: "text", text: message },
    ...images.map((image) => ({ type: "local_image" as const, path: image.path })),
  ];
}

function strictObjectSchema(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(strictObjectSchema);
  if (value === null || typeof value !== "object") return value;
  const mapped = Object.fromEntries(
    Object.entries(value).map(([key, item]) => [key, strictObjectSchema(item)]),
  );
  if (mapped.type === "object" && mapped.properties !== null && typeof mapped.properties === "object") {
    mapped.required = Object.keys(mapped.properties);
  }
  return mapped;
}

export function codexOutputSchema(schema: Record<string, unknown>): Record<string, unknown> {
  return strictObjectSchema(schema) as Record<string, unknown>;
}

export function claudeContent(
  message: string,
  images: readonly { path: string; data: Buffer }[],
): ContentBlockParam[] {
  return [
    { type: "text", text: message },
    ...images.map((image): ContentBlockParam => ({
      type: "image",
      source: {
        type: "base64",
        media_type: imageMime(image.path),
        data: image.data.toString("base64"),
      },
    })),
  ];
}

export function opencodeParts(
  message: string,
  schema: Record<string, unknown> | undefined,
  images: readonly ProviderImageAttachment[],
): Array<TextPartInput | FilePartInput> {
  const schemaParts: TextPartInput[] = schema === undefined
    ? []
    : [{ type: "text", text: `Return only JSON matching this JSON Schema:\n${JSON.stringify(schema)}` }];
  const imageParts: FilePartInput[] = images.map((image) => ({
    type: "file",
    mime: imageMime(image.path),
    filename: image.path.split("/").pop() ?? "image",
    url: pathToFileURL(image.path).href,
  }));
  return [
    { type: "text", text: message },
    ...schemaParts,
    ...imageParts,
  ];
}
