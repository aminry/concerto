import React, { useEffect, useState } from 'react';
import { StyleSheet, Text, View } from 'react-native';

import type { FpsMeter, FpsSample } from '@/perf/fps';
import { FPS_BUDGET, formatMs, RENDER_BUDGET_MS } from '@/perf/timing';
import { colors } from './theme';

interface PerfHudProps {
  readonly meter: FpsMeter;
  readonly renderMs: number | null;
  readonly buildMs: number | null;
  /** FlashList's own reported native draw time. */
  readonly drawMs: number | null;
  readonly rowCount: number | null;
  readonly fixtureLabel: string;
}

/**
 * Always-on overlay showing the two numbers the spike reports: time-to-first-
 * render (vs the 1.5 s budget) and live JS fps (vs the 60 fps budget). Colours
 * flip red when a number is outside budget so the operator can read the verdict
 * at a glance while profiling.
 */
export function PerfHud({
  meter,
  renderMs,
  buildMs,
  drawMs,
  rowCount,
  fixtureLabel,
}: PerfHudProps): React.ReactElement {
  const [sample, setSample] = useState<FpsSample>({ fps: 0, minFps: Infinity });

  useEffect(() => meter.subscribe(setSample), [meter]);

  const renderOk = renderMs === null || renderMs <= RENDER_BUDGET_MS;
  const fpsOk = sample.fps === 0 || sample.fps >= FPS_BUDGET - 2;
  const minFps = Number.isFinite(sample.minFps) ? sample.minFps : 0;

  return (
    <View style={styles.hud}>
      <Text style={styles.fixture}>{fixtureLabel}</Text>
      <View style={styles.row}>
        <Text style={styles.label}>render</Text>
        <Text style={[styles.value, renderOk ? styles.ok : styles.bad]}>
          {renderMs === null ? '—' : formatMs(renderMs)}
        </Text>
        <Text style={styles.budget}>/ 1.5 s</Text>
      </View>
      <View style={styles.row}>
        <Text style={styles.label}>build</Text>
        <Text style={styles.valueDim}>{buildMs === null ? '—' : formatMs(buildMs)}</Text>
        <Text style={styles.budget}>parse+flatten</Text>
      </View>
      <View style={styles.row}>
        <Text style={styles.label}>draw</Text>
        <Text style={styles.valueDim}>{drawMs === null ? '—' : formatMs(drawMs)}</Text>
        <Text style={styles.budget}>{rowCount === null ? '' : `${rowCount} rows`}</Text>
      </View>
      <View style={styles.row}>
        <Text style={styles.label}>fps</Text>
        <Text style={[styles.value, fpsOk ? styles.ok : styles.bad]}>{sample.fps || '—'}</Text>
        <Text style={styles.budget}>min {minFps || '—'} / 60</Text>
      </View>
      <Text style={styles.note}>JS fps — confirm on device profiler</Text>
    </View>
  );
}

const styles = StyleSheet.create({
  hud: {
    position: 'absolute',
    right: 8,
    top: 8,
    backgroundColor: 'rgba(13,17,23,0.92)',
    borderColor: colors.border,
    borderWidth: StyleSheet.hairlineWidth,
    borderRadius: 8,
    padding: 8,
    minWidth: 150,
  },
  fixture: { color: colors.accent, fontSize: 11, marginBottom: 4, fontWeight: '600' },
  row: { flexDirection: 'row', alignItems: 'baseline', marginBottom: 2 },
  label: { color: colors.dim, fontSize: 11, width: 40 },
  value: { fontSize: 14, fontWeight: '700', width: 56 },
  valueDim: { color: colors.dim, fontSize: 13, width: 56 },
  budget: { color: colors.gutter, fontSize: 10 },
  ok: { color: colors.add },
  bad: { color: colors.del },
  note: { color: colors.gutter, fontSize: 9, marginTop: 4, fontStyle: 'italic' },
});
