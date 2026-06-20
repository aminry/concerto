// Diff fixtures (Task 514) — shared by the parser/component tests and the
// `app/diff-demo.tsx` route. A representative multi-file unified diff plus a
// generator for the spike-103 1000-line perf fixture.

/**
 * A representative `git diff` over two files: one with an add/remove/context
 * hunk and a section heading, one a pure addition. Exercises file headers, two
 * hunks, all three body kinds, an empty context line, and a long line.
 */
export const SAMPLE_DIFF = `diff --git a/src/app.ts b/src/app.ts
index 1234567..89abcde 100644
--- a/src/app.ts
+++ b/src/app.ts
@@ -1,6 +1,7 @@ export function main()
 import { boot } from "./boot";

-const PORT = 3000;
+const PORT = Number(process.env.PORT ?? 3000);
+const HOST = "0.0.0.0";

 export function main() {
   boot();
@@ -20,3 +21,4 @@ export function main()
   console.log("started");
 }
+// trailing comment with a very long line that should scroll horizontally instead of wrapping onto the next visual line so the gutter stays aligned
 export default main;
diff --git a/README.md b/README.md
new file mode 100644
index 0000000..fedcba9
--- /dev/null
+++ b/README.md
@@ -0,0 +1,2 @@
+# Concerto
+A self-hosted agent orchestrator.
`;

/** Just the first file's diff (single-file rendering case). */
export const SINGLE_FILE_DIFF = `--- a/src/app.ts
+++ b/src/app.ts
@@ -1,3 +1,3 @@
 import { boot } from "./boot";
-const PORT = 3000;
+const PORT = 8080;
`;

/**
 * Build a large synthetic unified diff with ~`lines` changed body lines across
 * a single file/hunk — the spike-103 1000-line perf fixture. Each iteration
 * emits one context + one remove + one add line, so `lines` total ≈ 3×count
 * body rows; pass the desired BODY-LINE count.
 */
export function makeLargeDiff(lines = 1000): string {
  const per = Math.max(1, Math.floor(lines / 3));
  const parts: string[] = [
    "diff --git a/big.txt b/big.txt",
    "index 1111111..2222222 100644",
    "--- a/big.txt",
    "+++ b/big.txt",
    `@@ -1,${per * 2} +1,${per * 2} @@`,
  ];
  for (let i = 0; i < per; i++) {
    parts.push(` context line ${i} stays the same`);
    parts.push(`-old value at line ${i}`);
    parts.push(`+new value at line ${i}`);
  }
  return parts.join("\n") + "\n";
}
