import { FlashList } from '@shopify/flash-list';
import React, { useCallback, useMemo, useState } from 'react';
import * as Haptics from 'expo-haptics';
import { Platform } from 'react-native';

import { flattenDiff } from '@/diff/flatten';
import type { ParsedDiff, Row } from '@/diff/types';
import { ROW_HEIGHT } from './theme';
import { DiffRow } from './DiffRow';

interface DiffViewerProps {
  readonly diff: ParsedDiff;
  readonly syntax: boolean;
  /**
   * Fires once the list has drawn its first content frame. `drawMs` is
   * FlashList's own reported draw time (native), separate from the harness's
   * request-to-content wall clock.
   */
  readonly onFirstContent?: (rowCount: number, drawMs: number) => void;
}

/**
 * The virtualized diff renderer — the rendering approach the spike measures and
 * the one Task 514 would ship.
 *
 * Strategy: parse → flatten to one flat `Row[]` → render with FlashList, which
 * recycles row views and only mounts the visible window (plus a small overscan
 * buffer). A 10k-line diff therefore mounts ~30–50 rows at any time, not 10k.
 * Expand/collapse mutates a `collapsed` set and re-flattens; because tokens are
 * memoized (`flatten.ts`), re-flatten is cheap and the list diffs by stable
 * keys.
 *
 * Why FlashList over RN's `FlatList`: FlashList v2 is purpose-built for long,
 * uniform lists on the New Architecture and holds frame budget far better under
 * fast scroll, which is the exact thing the 60 fps bar tests.
 */
export function DiffViewer({ diff, syntax, onFirstContent }: DiffViewerProps): React.ReactElement {
  const [collapsed, setCollapsed] = useState<ReadonlySet<number>>(() => new Set());
  const reportedRef = React.useRef(false);

  const rows: Row[] = useMemo(
    () => flattenDiff(diff, { collapsed, syntax }),
    [diff, collapsed, syntax],
  );

  const onToggleFile = useCallback((fileIndex: number) => {
    if (Platform.OS !== 'web') {
      void Haptics.selectionAsync();
    }
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(fileIndex)) {
        next.delete(fileIndex);
      } else {
        next.add(fileIndex);
      }
      return next;
    });
  }, []);

  const onPressLine = useCallback((_key: string) => {
    // Production (Task 514): tap → context menu (copy / blame / open-in-desktop),
    // long-press → comment composer. The spike only needs the touch target wired
    // so hit-testing cost is represented.
  }, []);

  const renderItem = useCallback(
    ({ item }: { item: Row }) => (
      <DiffRow
        row={item}
        collapsed={item.type === 'file' ? collapsed.has(item.fileIndex) : false}
        onToggleFile={onToggleFile}
        onPressLine={onPressLine}
      />
    ),
    [collapsed, onToggleFile, onPressLine],
  );

  const keyExtractor = useCallback((item: Row) => item.key, []);

  // Distinct recycle pools per row kind so file/hunk/line views are never
  // reused as one another (cheaper, fewer layout thrashes under fast scroll).
  const getItemType = useCallback((item: Row) => item.type, []);

  const handleLoad = useCallback(
    (info: { elapsedTimeInMs: number }) => {
      if (!reportedRef.current) {
        reportedRef.current = true;
        onFirstContent?.(rows.length, info.elapsedTimeInMs);
      }
    },
    [onFirstContent, rows.length],
  );

  return (
    <FlashList
      data={rows}
      renderItem={renderItem}
      keyExtractor={keyExtractor}
      getItemType={getItemType}
      drawDistance={ROW_HEIGHT * 40}
      onLoad={handleLoad}
      // Long uniform list: disable the maintain-visible-content shift cost.
      maintainVisibleContentPosition={{ disabled: true }}
    />
  );
}
