// PreviewScreen tests (Task 517): requests a tunnel URL through the (mock)
// TunnelClient, feeds it to the WebView `source`, drives the WebView load
// loading/error states, and wires "Open in browser". The WebView is jest-mocked
// (jest.setup.ts) so real page loads stay Tier-3. RN-TL v13.3.3.
import { act, fireEvent, render, screen } from "@testing-library/react-native";

import { PreviewScreen } from "./PreviewScreen";
import { mockTunnelClient } from "./tunnel-client";

describe("PreviewScreen", () => {
  it("requests a tunnel URL and passes it to the WebView source", async () => {
    const client = mockTunnelClient();
    render(<PreviewScreen client={client} id="wa-aria" />);

    const webview = await screen.findByTestId("preview-webview");
    expect(webview.props.source).toEqual({ uri: "https://wa-aria.preview.concerto.localhost" });
    // The URL bar surfaces the same URL.
    expect(screen.getByTestId("preview-url")).toHaveTextContent(
      "https://wa-aria.preview.concerto.localhost",
    );
  });

  it("shows the tunnel loading state before the URL resolves", () => {
    jest.useFakeTimers();
    try {
      const client = mockTunnelClient({ delayMs: 1000 });
      const { unmount } = render(<PreviewScreen client={client} id="wa-aria" />);
      expect(screen.getByTestId("preview-tunnel-loading")).toBeOnTheScreen();
      unmount();
    } finally {
      jest.useRealTimers();
    }
  });

  it("shows the tunnel error state with a retry when the request rejects", async () => {
    const client = mockTunnelClient({ failWith: "no dev server running" });
    render(<PreviewScreen client={client} id="wa-aria" />);

    expect(await screen.findByTestId("preview-tunnel-error")).toBeOnTheScreen();
    expect(screen.getByText("no dev server running")).toBeOnTheScreen();
    expect(screen.getByTestId("preview-retry")).toBeOnTheScreen();
  });

  it("shows a WebView loading overlay until onLoadEnd fires, then hides it", async () => {
    const client = mockTunnelClient();
    render(<PreviewScreen client={client} id="wa-aria" />);

    const webview = await screen.findByTestId("preview-webview");
    // The overlay is visible while the page is still loading.
    expect(screen.getByTestId("preview-web-loading")).toBeOnTheScreen();

    act(() => {
      webview.props.onLoadEnd?.();
    });
    expect(screen.queryByTestId("preview-web-loading")).toBeNull();
  });

  it("shows a WebView error state when onError fires", async () => {
    const client = mockTunnelClient();
    render(<PreviewScreen client={client} id="wa-aria" />);

    const webview = await screen.findByTestId("preview-webview");
    act(() => {
      webview.props.onError?.();
    });
    expect(screen.getByTestId("preview-web-error")).toBeOnTheScreen();
    expect(screen.getByText("The preview page failed to load.")).toBeOnTheScreen();
  });

  it("opens the tunnel URL in the system browser via the injected opener", async () => {
    const client = mockTunnelClient();
    const openUrl = jest.fn(async () => true);
    render(<PreviewScreen client={client} id="wa-aria" openUrl={openUrl} />);

    // Wait for the URL to resolve so the button is enabled.
    await screen.findByTestId("preview-webview");
    fireEvent.press(screen.getByTestId("preview-open-browser"));
    expect(openUrl).toHaveBeenCalledWith("https://wa-aria.preview.concerto.localhost");
  });

  it("calls onBack when the back control is tapped", async () => {
    const client = mockTunnelClient();
    const onBack = jest.fn();
    render(<PreviewScreen client={client} id="wa-aria" onBack={onBack} />);

    await screen.findByTestId("preview-webview");
    fireEvent.press(screen.getByTestId("preview-back"));
    expect(onBack).toHaveBeenCalledTimes(1);
  });
});
