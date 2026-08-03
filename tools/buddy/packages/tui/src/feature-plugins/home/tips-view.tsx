import { createMemo, For } from "solid-js"
import { useTheme } from "../../context/theme"

type QuotePart = { text: string; highlight: boolean }

function parse(quote: string): QuotePart[] {
  const parts: QuotePart[] = []
  const regex = /\{highlight\}(.*?)\{\/highlight\}/g
  const found = Array.from(quote.matchAll(regex))
  const state = found.reduce(
    (acc, match) => {
      const start = match.index ?? 0
      if (start > acc.index) {
        acc.parts.push({ text: quote.slice(acc.index, start), highlight: false })
      }
      acc.parts.push({ text: match[1], highlight: true })
      acc.index = start + match[0].length
      return acc
    },
    { parts, index: 0 },
  )

  if (state.index < quote.length) {
    parts.push({ text: quote.slice(state.index), highlight: false })
  }

  return parts
}

export function Quotes() {
  const theme = useTheme().theme
  const quoteOffset = Math.random()
  const quote = createMemo(() => QUOTES[Math.floor(quoteOffset * QUOTES.length)] ?? DEFAULT_QUOTE, DEFAULT_QUOTE)
  const parts = createMemo(() => parse(quote()), DEFAULT_PARTS)

  return (
    <box flexDirection="row" maxWidth="100%">
      <text flexShrink={0} style={{ fg: theme.warning }}>
        ● Quote{" "}
      </text>
      <text flexShrink={1} wrapMode="word">
        <For each={parts()}>
          {(part) => <span style={{ fg: part.highlight ? theme.text : theme.textMuted }}>{part.text}</span>}
        </For>
      </text>
    </box>
  )
}

const QUOTES = [
  "Talk is cheap. Show me the code. {highlight}— Linus Torvalds{/highlight}",
  "Make everything as simple as possible, but not simpler. {highlight}— Albert Einstein{/highlight}",
  "Premature optimization is the root of all evil. {highlight}— Donald Knuth{/highlight}",
  "What I cannot create, I do not understand. {highlight}— Richard Feynman{/highlight}",
  "The limits of my language mean the limits of my world. {highlight}— Ludwig Wittgenstein{/highlight}",
  "Any fool can write code that a computer can understand. Good programmers write code that humans can understand. {highlight}— Martin Fowler{/highlight}",
  "The best way to predict the future is to invent it. {highlight}— Alan Kay{/highlight}",
]

const DEFAULT_QUOTE = QUOTES[0] ?? ""
const DEFAULT_PARTS = parse(DEFAULT_QUOTE)
