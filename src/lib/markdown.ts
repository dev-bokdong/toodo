const fence = /^\s*(```|~~~)/;
const rule = /^\s*([-*_])(\s*\1){2,}\s*$/;
const tableSeparator = /^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$/;
const blockMarkers = /^\s*(>\s*)*(#{1,6}\s+|[-*+]\s+|\d+[.)]\s+)?(\[[ xX]\]\s+)?/;

function inline(text: string): string {
  return (
    text
      // Link targets may hold one level of parentheses, e.g. javascript:alert(1).
      .replace(/!?\[([^\]]*)\]\((?:[^()]|\([^()]*\))*\)/g, "$1")
      .replace(/<((?:https?|mailto):[^>\s]+)>/g, "$1")
      .replace(/`+/g, "")
      // Only paired emphasis markers, leaving snake_case, __init__-like names, and a * b alone.
      .replace(/(\*\*|__|~~)(?=\S)(.+?)(?<=\S)\1/g, "$2")
      .replace(/(?<![\w*])([*_])(?=[^\s*_])(.+?)(?<=[^\s*_])\1(?![\w*])/g, "$2")
      .replace(/\s*\|\s*/g, " ")
  );
}

/** The text of a Markdown description without its syntax, on one line. */
export function markdownPreview(source: string): string {
  return source
    .split(/\r?\n/)
    .filter((line) => !fence.test(line) && !rule.test(line) && !tableSeparator.test(line))
    .map((line) => inline(line.replace(blockMarkers, "")))
    .join(" ")
    .replace(/\s+/g, " ")
    .trim();
}
