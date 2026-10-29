import { codeOnly } from "./graph.mjs";

/** Offsets come from comment/literal-erased Rust; bodies retain their original bytes. */
export function functions(source) {
  const code = codeOnly(source);
  return [...code.matchAll(/\bfn\s+(\w+)\s*(?:<[^{}]*>)?\s*\(/g)].flatMap((match) => {
    const start = code.indexOf("{", match.index);
    if (start < 0) return [];
    let end = start + 1;
    let depth = 1;
    while (end < code.length && depth) {
      if (code[end] === "{") depth++;
      if (code[end] === "}") depth--;
      end++;
    }
    const preceding = code.slice(code.lastIndexOf("}", match.index) + 1, match.index);
    return [
      {
        name: match[1],
        signature: source.slice(match.index, start),
        start: match.index,
        end,
        test: /#\s*\[\s*test\s*\]/.test(preceding),
        body: source.slice(start, end),
        code: code.slice(start, end),
      },
    ];
  });
}

/** Split call arguments at balanced delimiters, preserving literal bytes for argv attribution. */
export function calls(text) {
  const code = codeOnly(text);
  const result = [];
  for (const match of code.matchAll(/\b(\w+)\s*\(/g)) {
    const open = code.indexOf("(", match.index);
    const args = [];
    let start = open + 1;
    let parentheses = 1;
    let brackets = 0;
    let braces = 0;
    let at = start;
    for (; at < code.length && parentheses; at++) {
      const char = code[at];
      if (char === "(") parentheses++;
      else if (char === ")") parentheses--;
      else if (char === "[") brackets++;
      else if (char === "]") brackets--;
      else if (char === "{") braces++;
      else if (char === "}") braces--;
      if ((char === "," && parentheses === 1 && !brackets && !braces) || !parentheses) {
        args.push(text.slice(start, at).trim());
        start = at + 1;
      }
    }
    result.push({
      name: match[1],
      at: match.index,
      method: code[match.index - 1] === ".",
      args,
      end: at,
    });
  }
  return result;
}
