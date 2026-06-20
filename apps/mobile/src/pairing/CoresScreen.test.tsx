// CoresScreen tests (Task 511, Tier-2). Lists paired Cores, marks the active
// one, and exercises switch / remove / pair-another through an injected registry
// (deterministic, no secure-store needed). RN-TL v13.3.3.
import { fireEvent, render, screen, waitFor } from "@testing-library/react-native";

import { CoresScreen } from "./CoresScreen";
import type { StoredCore } from "./core-store";

function core(id: string, label: string): StoredCore {
  return {
    id,
    label,
    blob: { endpointId: id, directAddrs: ["1.2.3.4:1"], coreNoisePub: "b".repeat(64) },
    deviceIdHex: "aa",
    pairedAtMs: 0,
  };
}

function makeRegistry(initial: StoredCore[], activeId: string | null) {
  const state = { cores: [...initial], activeId };
  return {
    state,
    listCores: jest.fn(async () => state.cores),
    activeCoreId: jest.fn(async () => state.activeId),
    switchCore: jest.fn(async (id: string) => {
      state.activeId = id;
    }),
    removeCore: jest.fn(async (id: string) => {
      state.cores = state.cores.filter((c) => c.id !== id);
      if (state.activeId === id) state.activeId = state.cores[0]?.id ?? null;
    }),
  };
}

describe("CoresScreen", () => {
  it("lists paired Cores and marks the active one", async () => {
    const reg = makeRegistry([core("a", "Core A"), core("b", "Core B")], "b");
    render(<CoresScreen registry={reg} />);

    expect(await screen.findByTestId("cores-list")).toBeOnTheScreen();
    expect(screen.getByText("Core A")).toBeOnTheScreen();
    expect(screen.getByText("Core B")).toBeOnTheScreen();
    // The active row labels itself "Active".
    expect(screen.getByTestId("core-row-b")).toBeOnTheScreen();
    expect(screen.getByText("Active")).toBeOnTheScreen();
  });

  it("switches the active Core on tap", async () => {
    const reg = makeRegistry([core("a", "Core A"), core("b", "Core B")], "b");
    render(<CoresScreen registry={reg} />);
    await screen.findByTestId("core-row-a");
    // The switch pressable carries an a11y label "Use Core A".
    fireEvent.press(screen.getByLabelText("Use Core A"));
    await waitFor(() => expect(reg.switchCore).toHaveBeenCalledWith("a"));
  });

  it("removes a Core", async () => {
    const reg = makeRegistry([core("a", "Core A"), core("b", "Core B")], "b");
    render(<CoresScreen registry={reg} />);
    fireEvent.press(await screen.findByTestId("core-remove-b"));
    await waitFor(() => expect(reg.removeCore).toHaveBeenCalledWith("b"));
    // After reload, Core B is gone.
    await waitFor(() => expect(screen.queryByText("Core B")).toBeNull());
  });

  it("shows the empty state and a pair affordance", async () => {
    const reg = makeRegistry([], null);
    const onPairAnother = jest.fn();
    render(<CoresScreen registry={reg} onPairAnother={onPairAnother} />);
    expect(await screen.findByTestId("cores-empty")).toBeOnTheScreen();
    fireEvent.press(screen.getByTestId("cores-pair-empty"));
    expect(onPairAnother).toHaveBeenCalled();
  });

  it("routes to pair-another from the list footer", async () => {
    const reg = makeRegistry([core("a", "Core A")], "a");
    const onPairAnother = jest.fn();
    render(<CoresScreen registry={reg} onPairAnother={onPairAnother} />);
    fireEvent.press(await screen.findByTestId("cores-pair-another"));
    expect(onPairAnother).toHaveBeenCalled();
  });
});
