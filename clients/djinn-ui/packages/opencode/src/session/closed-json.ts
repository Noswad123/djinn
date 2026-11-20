// JSON.parse cannot detect duplicate keys. Validate object key tokens first,
// including escaped spellings, without changing JSON's value semantics.
export function parse(text: string): unknown {
  const stack: (Set<string> | null)[] = []
  const tokens = /"(?:[^"\\]|\\.)*"|[{}\[\]:,]|[^\s{}\[\]:,]+/g
  let previous = ""
  for (const match of text.matchAll(tokens)) {
    const token = match[0]
    if (token === "{" || token === "[") stack.push(token === "{" ? new Set() : null)
    else if (token === "}" || token === "]") stack.pop()
    else if (token.startsWith('"') && (previous === "{" || previous === ",") && stack.at(-1)) {
      const key = JSON.parse(token) as string
      if (stack.at(-1)!.has(key)) throw new Error("duplicate JSON key")
      stack.at(-1)!.add(key)
    }
    previous = token
  }
  return JSON.parse(text)
}

export * as ClosedJson from "./closed-json"
