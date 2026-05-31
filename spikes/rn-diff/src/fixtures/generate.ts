/**
 * Programmatic unified-diff fixture generator.
 *
 * Generates representative `git diff`-shaped text with realistic code-ish
 * content (so the syntax tokenizer has real work to do) at controllable line
 * counts. Two sizes are exported for the harness:
 *
 *   - `~1000-line` diff  — the V1.0 budget target (`design/16 §10`).
 *   - `~10000-line` diff — the large diff used to find the performance cliff.
 *
 * Generated rather than committed-static so the cliff size can be tuned and so
 * the repo does not carry a megabyte of fixture text. Deterministic (seeded)
 * so reruns are comparable.
 */

const LANG_SNIPPETS: readonly string[] = [
  'const handler = (req: Request, res: Response): void => {',
  '  const token = req.headers["authorization"]?.slice(7) ?? null;',
  '  if (token === null) { return res.status(401).json({ error: "no auth" }); }',
  '  // verify the device certificate against the issuer chain',
  '  const cert = await verifyDeviceCert(token, issuerPubKey);',
  '  logger.info(`device ${cert.deviceId} authenticated in ${elapsed}ms`);',
  'pub fn flatten_diff(diff: &ParsedDiff, collapsed: &HashSet<usize>) -> Vec<Row> {',
  '    let mut rows = Vec::with_capacity(diff.total_lines + diff.files.len() * 2);',
  '    for (f, file) in diff.files.iter().enumerate() {',
  '        rows.push(Row::File { path: file.path.clone(), index: f });',
  '        match self.kind { LineKind::Add => count += 1, _ => {} }',
  'def summarize(workarea: Workarea, budget_tokens: int = 600) -> str:',
  '    digest = []  # grouped by repo, capped at the daily budget',
  '    for repo in workarea.repos:',
  '        if repo.excluded_from_maestro: continue',
  '    return "\\n".join(digest)[:budget_tokens]',
  'func (s *Server) Subscribe(req *pb.SubscribeRequest, stream pb.Streams_SubscribeServer) error {',
  '\tring := s.buffers.GetOrCreate(req.StreamId)',
  '\treturn ring.Replay(req.SinceOffset, func(ev *pb.Event) error { return stream.Send(ev) })',
  'interface SessionHandle { readonly id: string; readonly endpointId: string; }',
  'export async function openSession(coreEndpointId: string): Promise<SessionHandle> {',
  '  // Iroh native module: Rust -> JSI (iOS) / Rust -> JNI (Android)',
  '  const handle = await ConcertoIroh.openSession(coreEndpointId);',
  '  return { id: handle.id, endpointId: coreEndpointId };',
  '',
  '}',
];

const FILE_PATHS: readonly string[] = [
  'crates/transport/src/iroh_endpoint.rs',
  'crates/core/src/services/streams.rs',
  'apps/mobile/src/diff/renderer.tsx',
  'apps/mobile/src/transport/session.ts',
  'packages/proto-client/src/notifications.ts',
  'crates/maestro/src/digest.rs',
  'apps/web/src/data/client.ts',
  'crates/relay/src/wss_bridge.rs',
];

// Small deterministic PRNG (mulberry32) — stable fixtures across runs.
function makeRng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function snippet(rng: () => number): string {
  const idx = Math.floor(rng() * LANG_SNIPPETS.length);
  return LANG_SNIPPETS[idx] ?? '';
}

/**
 * Generate a unified diff with approximately `targetLines` changed/context
 * lines spread across several files and hunks.
 */
export function generateUnifiedDiff(targetLines: number, seed = 1): string {
  const rng = makeRng(seed);
  const out: string[] = [];
  let produced = 0;
  let fileIdx = 0;

  // ~40 changed lines per hunk, ~6 hunks per file → ~240 lines/file.
  const linesPerHunk = 40;
  const hunksPerFile = 6;

  while (produced < targetLines) {
    const path = FILE_PATHS[fileIdx % FILE_PATHS.length] ?? 'src/file.ts';
    const variant = Math.floor(fileIdx / FILE_PATHS.length);
    const fullPath = variant === 0 ? path : path.replace(/(\.\w+)$/, `_${variant}$1`);
    fileIdx++;

    out.push(`diff --git a/${fullPath} b/${fullPath}`);
    out.push('index 1a2b3c4..5d6e7f8 100644');
    out.push(`--- a/${fullPath}`);
    out.push(`+++ b/${fullPath}`);

    let oldStart = 10 + Math.floor(rng() * 40);
    let newStart = oldStart;

    for (let h = 0; h < hunksPerFile && produced < targetLines; h++) {
      const oldLen = linesPerHunk;
      const newLen = linesPerHunk;
      out.push(
        `@@ -${oldStart},${oldLen} +${newStart},${newLen} @@ ${snippet(rng).slice(0, 24)}`,
      );

      for (let i = 0; i < linesPerHunk && produced < targetLines; i++) {
        const roll = rng();
        if (roll < 0.35) {
          out.push(`+${snippet(rng)}`);
        } else if (roll < 0.6) {
          out.push(`-${snippet(rng)}`);
        } else {
          out.push(` ${snippet(rng)}`);
        }
        produced++;
      }

      oldStart += oldLen + 8;
      newStart += newLen + 8;
    }
  }

  return out.join('\n');
}

export const SMALL_DIFF_TARGET = 1000;
export const LARGE_DIFF_TARGET = 10000;
