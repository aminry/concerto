// createMockRecognizer tests (Task 515, Tier-2). Proves the recognizer seam's
// idle->listening->partial->final flow and the permission/error paths a real STT
// engine (Tier-3) would drive.
import { createMockRecognizer } from "./speech-recognizer";

describe("createMockRecognizer", () => {
  it("emits partials on start then a final on stop", async () => {
    const partials: string[] = [];
    let final: string | undefined;
    const rec = createMockRecognizer({
      partials: ["book", "book a"],
      final: "book a meeting",
    });

    await rec.start({
      onPartial: (t) => partials.push(t),
      onFinal: (t) => {
        final = t;
      },
    });
    expect(partials).toEqual(["book", "book a"]);
    expect(final).toBeUndefined();

    await rec.stop();
    expect(final).toBe("book a meeting");
  });

  it("walks partial->final on a single start when autoFinal", async () => {
    const partials: string[] = [];
    let final: string | undefined;
    const rec = createMockRecognizer(
      { partials: ["hello"], final: "hello world" },
      { autoFinal: true },
    );
    await rec.start({
      onPartial: (t) => partials.push(t),
      onFinal: (t) => {
        final = t;
      },
    });
    expect(partials).toEqual(["hello"]);
    expect(final).toBe("hello world");
  });

  it("reports the configured permission status", async () => {
    const granted = createMockRecognizer({}, { permission: "granted" });
    const denied = createMockRecognizer({}, { permission: "denied" });
    await expect(granted.requestPermission()).resolves.toBe("granted");
    await expect(denied.requestPermission()).resolves.toBe("denied");
  });

  it("reports availability (so the UI can hide the mic)", () => {
    expect(createMockRecognizer().isAvailable()).toBe(true);
    expect(createMockRecognizer({}, { available: false }).isAvailable()).toBe(false);
  });

  it("surfaces a start error via onError instead of listening", async () => {
    let err: string | undefined;
    const partials: string[] = [];
    const rec = createMockRecognizer(
      { partials: ["x"] },
      { startError: "no speech detected" },
    );
    await rec.start({
      onPartial: (t) => partials.push(t),
      onError: (m) => {
        err = m;
      },
    });
    expect(err).toBe("no speech detected");
    expect(partials).toEqual([]);
  });
});
