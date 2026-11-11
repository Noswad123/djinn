export * as EventStorage from "./storage"

import { deflateSync, inflateSync } from "node:zlib"

export const PREFIX = "djinn:event:v1:zlib:"
export const MAX_BYTES = 64 * 1024 * 1024
export const MIN_BYTES = 1024

export type Options = { compression?: boolean }

export function encode(input: Record<string, unknown>, options?: Options): string {
  return pack(JSON.stringify(input), options)
}

// Maintenance must use the raw API rather than parse/stringify historical JSON.
export function encodeRaw(input: string, options?: Options): string {
  JSON.parse(input)
  return pack(input, options)
}

export function decode(input: string): Record<string, unknown> {
  return JSON.parse(unpack(input))
}

export function decodeRaw(input: string): string {
  const output = unpack(input)
  JSON.parse(output)
  return output
}

function pack(input: string, options?: Options): string {
  if (!(options?.compression ?? process.env.DJINN_EVENT_COMPRESSION === "1")) return input
  const bytes = Buffer.byteLength(input, "utf8")
  if (bytes < MIN_BYTES || bytes > MAX_BYTES) return input
  const raw = Buffer.from(input, "utf8")
  // A JS string can contain unpaired surrogates that UTF-8 would replace.
  if (raw.toString("utf8") !== input) return input
  const output = `${PREFIX}${bytes}:${deflateSync(raw).toString("base64")}`
  return output.length <= bytes - Math.max(128, Math.ceil(bytes * 0.1)) ? output : input
}

function unpack(input: string): string {
  if (!input.startsWith("djinn:event:")) return input
  if (!input.startsWith(PREFIX)) throw new Error("Unsupported event storage version or codec")
  const separator = input.indexOf(":", PREFIX.length)
  const size = input.slice(PREFIX.length, separator)
  if (separator === -1 || !/^[1-9][0-9]{0,8}$/.test(size)) throw new Error("Invalid event storage length")
  const length = Number(size)
  if (length > MAX_BYTES) throw new Error("Event storage payload exceeds decompression limit")
  const encoded = input.slice(separator + 1)
  // Buffer.from is permissive; require canonical padded standard base64 first.
  if (
    !encoded.length ||
    encoded.length > 4 * Math.ceil((MAX_BYTES + 65536) / 3) ||
    encoded.length % 4 !== 0 ||
    !/^[A-Za-z0-9+/]*={0,2}$/.test(encoded)
  ) {
    throw new Error("Invalid event storage base64")
  }
  const compressed = Buffer.from(encoded, "base64")
  if (compressed.toString("base64") !== encoded) throw new Error("Noncanonical event storage base64")
  // info exposes consumed input bytes, rejecting appended garbage or extra streams.
  const result: unknown = inflateSync(compressed, { maxOutputLength: length, info: true })
  // Node's type definitions omit the info overload. Validate its runtime shape
  // rather than assuming that Bun and Node returned a complete inflate result.
  if (
    !result ||
    typeof result !== "object" ||
    !("buffer" in result) ||
    !Buffer.isBuffer(result.buffer) ||
    !("engine" in result) ||
    !result.engine ||
    typeof result.engine !== "object" ||
    !("bytesWritten" in result.engine) ||
    typeof result.engine.bytesWritten !== "number"
  ) {
    throw new Error("Invalid event storage decompression result")
  }
  if (result.engine.bytesWritten !== compressed.length) throw new Error("Trailing event storage compressed data")
  if (result.buffer.length !== length) throw new Error("Event storage payload length mismatch")
  return new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(result.buffer)
}
