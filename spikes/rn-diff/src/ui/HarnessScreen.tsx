import React, { useCallback, useMemo, useRef, useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { parseUnifiedDiff } from '@/diff/parse';
import type { ParsedDiff } from '@/diff/types';
import {
  generateUnifiedDiff,
  LARGE_DIFF_TARGET,
  SMALL_DIFF_TARGET,
} from '@/fixtures/generate';
import { FpsMeter } from '@/perf/fps';
import { nowMs } from '@/perf/timing';
import { colors } from './theme';
import { DiffViewer } from './DiffViewer';
import { PerfHud } from './PerfHud';

type FixtureId = 'small' | 'large';

interface Fixture {
  readonly id: FixtureId;
  readonly label: string;
  readonly target: number;
}

const FIXTURES: readonly Fixture[] = [
  { id: 'small', label: '~1k lines', target: SMALL_DIFF_TARGET },
  { id: 'large', label: '~10k lines', target: LARGE_DIFF_TARGET },
];

interface LoadedFixture {
  readonly id: FixtureId;
  readonly diff: ParsedDiff;
  readonly buildMs: number;
  /** Monotonic timestamp captured at request time (for time-to-render). */
  readonly requestedAt: number;
  readonly nonce: number;
}

/**
 * The harness screen.
 *
 * Flow the operator drives on a real device:
 *  1. Tap a fixture button (~1k / ~10k). We timestamp the press, then
 *     synchronously parse + flatten + tokenize and mount the DiffViewer.
 *  2. DiffViewer fires `onFirstContent` after its first window commits; the
 *     elapsed span is the reported time-to-render.
 *  3. Flick-scroll hard and read the fps row (and the worst-case `min`). For the
 *     authoritative 60 fps verdict, attach Xcode Instruments (Core Animation)
 *     or Android GPU profiler — see the findings doc.
 *  4. Tap "syntax" to toggle the tokenizer (isolates highlight cost) and tap
 *     file headers to expand/collapse hunks.
 */
export function HarnessScreen(): React.ReactElement {
  const insets = useSafeAreaInsets();
  const meterRef = useRef<FpsMeter>(new FpsMeter());
  const [syntax, setSyntax] = useState(true);
  const [loaded, setLoaded] = useState<LoadedFixture | null>(null);
  const [renderMs, setRenderMs] = useState<number | null>(null);
  const [drawMs, setDrawMs] = useState<number | null>(null);
  const [rowCount, setRowCount] = useState<number | null>(null);
  const nonceRef = useRef(0);

  // Start the fps meter once.
  React.useEffect(() => {
    const meter = meterRef.current;
    meter.start();
    return () => meter.stop();
  }, []);

  const loadFixture = useCallback((fixture: Fixture) => {
    const requestedAt = nowMs();
    setRenderMs(null);
    setDrawMs(null);
    setRowCount(null);
    meterRef.current.reset();

    const buildStart = nowMs();
    const raw = generateUnifiedDiff(fixture.target, 7);
    const diff = parseUnifiedDiff(raw);
    const buildMs = nowMs() - buildStart;

    nonceRef.current += 1;
    setLoaded({ id: fixture.id, diff, buildMs, requestedAt, nonce: nonceRef.current });
  }, []);

  const onFirstContent = useCallback((count: number, listDrawMs: number) => {
    setLoaded((cur) => {
      if (cur) {
        setRenderMs(nowMs() - cur.requestedAt);
      }
      return cur;
    });
    setDrawMs(listDrawMs);
    setRowCount(count);
  }, []);

  const fixtureLabel = useMemo(() => {
    if (!loaded) {
      return 'no fixture';
    }
    const f = FIXTURES.find((x) => x.id === loaded.id);
    return `${f?.label ?? ''} · ${syntax ? 'syntax' : 'plain'}`;
  }, [loaded, syntax]);

  return (
    <View style={[styles.root, { paddingTop: insets.top }]}>
      <View style={styles.toolbar}>
        <Text style={styles.title}>RN diff perf spike</Text>
        <View style={styles.buttons}>
          {FIXTURES.map((f) => (
            <Pressable
              key={f.id}
              style={[styles.btn, loaded?.id === f.id && styles.btnActive]}
              onPress={() => loadFixture(f)}
            >
              <Text style={styles.btnText}>{f.label}</Text>
            </Pressable>
          ))}
          <Pressable
            style={[styles.btn, syntax && styles.btnActive]}
            onPress={() => setSyntax((s) => !s)}
          >
            <Text style={styles.btnText}>syntax</Text>
          </Pressable>
        </View>
      </View>

      <View style={styles.body}>
        {loaded ? (
          <DiffViewer
            // Remount on fixture/syntax change so time-to-render is measured fresh.
            key={`${loaded.nonce}-${syntax}`}
            diff={loaded.diff}
            syntax={syntax}
            onFirstContent={onFirstContent}
          />
        ) : (
          <View style={styles.empty}>
            <Text style={styles.emptyText}>
              Tap a fixture to render a representative unified diff.
            </Text>
            <Text style={styles.emptyHint}>
              Then flick-scroll and read the fps. Tap file headers to
              expand/collapse hunks.
            </Text>
          </View>
        )}
      </View>

      <PerfHud
        meter={meterRef.current}
        renderMs={renderMs}
        buildMs={loaded?.buildMs ?? null}
        drawMs={drawMs}
        rowCount={rowCount}
        fixtureLabel={fixtureLabel}
      />
    </View>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1, backgroundColor: colors.bg },
  toolbar: {
    paddingHorizontal: 12,
    paddingBottom: 8,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderColor: colors.border,
  },
  title: { color: colors.text, fontSize: 16, fontWeight: '700', marginBottom: 8 },
  buttons: { flexDirection: 'row', gap: 8 },
  btn: {
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderRadius: 6,
    backgroundColor: colors.panel,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: colors.border,
  },
  btnActive: { borderColor: colors.accent, backgroundColor: '#1f2733' },
  btnText: { color: colors.text, fontSize: 13 },
  body: { flex: 1 },
  empty: { flex: 1, alignItems: 'center', justifyContent: 'center', padding: 24 },
  emptyText: { color: colors.text, fontSize: 15, textAlign: 'center' },
  emptyHint: { color: colors.dim, fontSize: 13, textAlign: 'center', marginTop: 8 },
});
