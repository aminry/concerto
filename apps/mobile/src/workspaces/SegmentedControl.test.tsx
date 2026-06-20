// SegmentedControl tests (Task 513): a JS-only (Tier-2) tab switcher. Renders
// its options, reflects the selected state for a11y, and fires onChange on tap.
import { fireEvent, render, screen } from "@testing-library/react-native";

import { SegmentedControl } from "./SegmentedControl";

describe("SegmentedControl", () => {
  const options = [
    { value: "a" as const, label: "Alpha" },
    { value: "b" as const, label: "Beta" },
  ];

  it("renders each option and marks the selected one", () => {
    render(<SegmentedControl options={options} value="a" onChange={() => {}} testIDPrefix="seg" />);
    expect(screen.getByText("Alpha")).toBeOnTheScreen();
    expect(screen.getByText("Beta")).toBeOnTheScreen();
    expect(screen.getByTestId("seg-a")).toBeSelected();
    expect(screen.getByTestId("seg-b")).not.toBeSelected();
  });

  it("calls onChange with the tapped value", () => {
    const onChange = jest.fn();
    render(<SegmentedControl options={options} value="a" onChange={onChange} testIDPrefix="seg" />);
    fireEvent.press(screen.getByTestId("seg-b"));
    expect(onChange).toHaveBeenCalledWith("b");
  });
});
