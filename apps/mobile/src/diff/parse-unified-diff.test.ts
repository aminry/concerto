// Parser tests (Task 514): unified-diff text -> typed rows. Covers file/hunk
// headers, add/remove/context body rows, line-number tracking, the section
// heading, a rename, /dev/null new files, and the 1000-line perf fixture shape.
import {
  parseUnifiedDiff,
  summarizeRows,
  type AddRow,
  type ContextRow,
  type RemoveRow,
} from "./parse-unified-diff";
import { SAMPLE_DIFF, SINGLE_FILE_DIFF, makeLargeDiff } from "./diff-fixtures";

describe("parseUnifiedDiff", () => {
  it("turns a single-file diff into file + hunk + body rows", () => {
    const rows = parseUnifiedDiff(SINGLE_FILE_DIFF);
    const kinds = rows.map((r) => r.kind);
    expect(kinds).toEqual(["file", "hunk", "context", "remove", "add"]);

    const file = rows[0];
    expect(file.kind).toBe("file");
    if (file.kind === "file") expect(file.path).toBe("src/app.ts");
  });

  it("classifies add/remove/context lines and strips the marker char", () => {
    const rows = parseUnifiedDiff(SINGLE_FILE_DIFF);
    const ctx = rows.find((r) => r.kind === "context") as ContextRow;
    const rem = rows.find((r) => r.kind === "remove") as RemoveRow;
    const add = rows.find((r) => r.kind === "add") as AddRow;

    expect(ctx.content).toBe('import { boot } from "./boot";');
    expect(rem.content).toBe("const PORT = 3000;");
    expect(add.content).toBe("const PORT = 8080;");
  });

  it("tracks old/new line numbers across the hunk", () => {
    const rows = parseUnifiedDiff(SINGLE_FILE_DIFF);
    const ctx = rows.find((r) => r.kind === "context") as ContextRow;
    const rem = rows.find((r) => r.kind === "remove") as RemoveRow;
    const add = rows.find((r) => r.kind === "add") as AddRow;

    // @@ -1,3 +1,3 @@ — context is line 1 both sides.
    expect(ctx.oldLine).toBe(1);
    expect(ctx.newLine).toBe(1);
    // remove is old line 2; add is new line 2.
    expect(rem.oldLine).toBe(2);
    expect(add.newLine).toBe(2);
  });

  it("parses a multi-file diff with two files, two hunks and a section heading", () => {
    const rows = parseUnifiedDiff(SAMPLE_DIFF);
    const summary = summarizeRows(rows);
    expect(summary.files).toBe(2);
    expect(summary.hunks).toBe(3);
    expect(summary.added).toBeGreaterThan(0);
    expect(summary.removed).toBeGreaterThan(0);

    // The first hunk header carries the section heading after the second @@.
    const firstHunk = rows.find((r) => r.kind === "hunk");
    expect(firstHunk?.kind).toBe("hunk");
    if (firstHunk?.kind === "hunk") {
      expect(firstHunk.header).toBe("@@ -1,6 +1,7 @@");
      expect(firstHunk.section).toBe("export function main()");
    }
  });

  it("handles a /dev/null new file (README) — path comes from the +++ side", () => {
    const rows = parseUnifiedDiff(SAMPLE_DIFF);
    const readme = rows.find((r) => r.kind === "file" && r.path === "README.md");
    expect(readme).toBeDefined();
    if (readme?.kind === "file") {
      // oldPath is /dev/null for a new file -> we don't surface it as a rename.
      expect(readme.oldPath === undefined || readme.oldPath === "/dev/null").toBe(true);
    }
  });

  it("surfaces a rename as oldPath -> path", () => {
    const renamed = `diff --git a/old/name.ts b/new/name.ts
similarity index 90%
rename from old/name.ts
rename to new/name.ts
--- a/old/name.ts
+++ b/new/name.ts
@@ -1,1 +1,1 @@
-x
+y
`;
    const rows = parseUnifiedDiff(renamed);
    const file = rows.find((r) => r.kind === "file");
    if (file?.kind === "file") {
      expect(file.path).toBe("new/name.ts");
      expect(file.oldPath).toBe("old/name.ts");
    } else {
      throw new Error("expected a file row");
    }
  });

  it("gives every row a unique key", () => {
    const rows = parseUnifiedDiff(SAMPLE_DIFF);
    const keys = new Set(rows.map((r) => r.key));
    expect(keys.size).toBe(rows.length);
  });

  it("ignores '\\ No newline at end of file' annotations", () => {
    const diff = `--- a/f
+++ b/f
@@ -1,1 +1,1 @@
-a
\\ No newline at end of file
+b
\\ No newline at end of file
`;
    const rows = parseUnifiedDiff(diff);
    const summary = summarizeRows(rows);
    expect(summary.added).toBe(1);
    expect(summary.removed).toBe(1);
    // No stray rows for the backslash annotations.
    expect(rows.every((r) => !r.key.includes("\\"))).toBe(true);
  });

  it("parses an empty input to zero rows", () => {
    expect(parseUnifiedDiff("")).toEqual([]);
  });

  it("parses a 1000-body-line fixture without error (perf-fixture shape)", () => {
    const rows = parseUnifiedDiff(makeLargeDiff(1000));
    const summary = summarizeRows(rows);
    // ~333 of each body kind for a 1000-line budget.
    expect(summary.added).toBeGreaterThanOrEqual(330);
    expect(summary.removed).toBeGreaterThanOrEqual(330);
    expect(summary.context).toBeGreaterThanOrEqual(330);
    expect(summary.files).toBe(1);
    expect(summary.hunks).toBe(1);
  });
});
