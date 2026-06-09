// Auto-generated workspace name from selected repo names (design Part 1,
// format A). Names arrive in selection order.
//
//   []            -> ""
//   [a]           -> "a"
//   [a, b]        -> "a + b"
//   [a, b, c,...] -> "a + b + N more"   (N = count - 2)

export function deriveWorkspaceName(names: string[]): string {
  if (names.length === 0) return "";
  if (names.length === 1) return names[0];
  if (names.length === 2) return `${names[0]} + ${names[1]}`;
  const more = names.length - 2;
  return `${names[0]} + ${names[1]} + ${more} more`;
}
