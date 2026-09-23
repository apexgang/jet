// @vitest-environment happy-dom
import { cleanup, fireEvent, render } from "@testing-library/svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import ColumnResizer from "../src/lib/features/shell/ColumnResizer.svelte";

afterEach(() => cleanup());

function separator(edge: "start" | "end", value = 244, onchange = vi.fn()) {
  const range = edge === "start" ? { min: 210, ideal: 244, max: 300 } : { min: 280, ideal: 340, max: 440 };
  const { container, rerender } = render(ColumnResizer, {
    label: edge === "start" ? "Resize sidebar" : "Resize work panel",
    value,
    edge,
    onchange,
    ...range,
  });
  const element = container.querySelector<HTMLElement>('[role="separator"]')!;
  return { element, onchange, rerender };
}

describe("ColumnResizer", () => {
  it("is a focusable vertical separator carrying its range and width", () => {
    const { element } = separator("start", 250);
    expect(element.getAttribute("aria-orientation")).toBe("vertical");
    expect(element.getAttribute("aria-label")).toBe("Resize sidebar");
    expect(element.getAttribute("aria-valuemin")).toBe("210");
    expect(element.getAttribute("aria-valuemax")).toBe("300");
    expect(element.getAttribute("aria-valuenow")).toBe("250");
    expect(element.tabIndex).toBe(0);
  });

  it("follows value changes", async () => {
    const { element, rerender } = separator("end", 340);
    await rerender({ value: 400 });
    expect(element.getAttribute("aria-valuenow")).toBe("400");
  });

  it("start edge: ArrowRight widens, ArrowLeft narrows, Shift takes 32 px", async () => {
    const { element, onchange } = separator("start", 244);
    await fireEvent.keyDown(element, { key: "ArrowRight" });
    await fireEvent.keyDown(element, { key: "ArrowLeft" });
    await fireEvent.keyDown(element, { key: "ArrowRight", shiftKey: true });
    await fireEvent.keyDown(element, { key: "ArrowUp" });
    await fireEvent.keyDown(element, { key: "ArrowDown" });
    expect(onchange.mock.calls.map(([width]) => width)).toEqual([252, 236, 276, 252, 236]);
  });

  it("end edge: ArrowLeft widens and ArrowRight narrows", async () => {
    const { element, onchange } = separator("end", 340);
    await fireEvent.keyDown(element, { key: "ArrowLeft" });
    await fireEvent.keyDown(element, { key: "ArrowRight" });
    await fireEvent.keyDown(element, { key: "ArrowLeft", shiftKey: true });
    expect(onchange.mock.calls.map(([width]) => width)).toEqual([348, 332, 372]);
  });

  it("Home and End jump to the range ends", async () => {
    const { element, onchange } = separator("end", 340);
    await fireEvent.keyDown(element, { key: "Home" });
    await fireEvent.keyDown(element, { key: "End" });
    expect(onchange.mock.calls.map(([width]) => width)).toEqual([280, 440]);
  });

  it("passes only clamped values and nothing when the width would not change", async () => {
    const { element, onchange } = separator("start", 296);
    await fireEvent.keyDown(element, { key: "ArrowRight", shiftKey: true });
    expect(onchange).toHaveBeenLastCalledWith(300);
    onchange.mockClear();
    const atMax = separator("start", 300);
    await fireEvent.keyDown(atMax.element, { key: "End" });
    await fireEvent.keyDown(atMax.element, { key: "ArrowRight" });
    expect(atMax.onchange).not.toHaveBeenCalled();
  });

  it("ignores unrelated keys and modified arrows, leaving them to the page", async () => {
    const { element, onchange } = separator("start", 244);
    const tab = await fireEvent.keyDown(element, { key: "Tab" });
    const ctrl = await fireEvent.keyDown(element, { key: "ArrowRight", ctrlKey: true });
    expect(tab).toBe(true);
    expect(ctrl).toBe(true);
    expect(onchange).not.toHaveBeenCalled();
  });

  it("double-click resets to the ideal width", async () => {
    const { element, onchange } = separator("end", 420);
    await fireEvent.dblClick(element);
    expect(onchange).toHaveBeenCalledWith(340);
  });

  it("follows a pointer drag in the direction of its edge, clamped", async () => {
    const start = separator("start", 244);
    await fireEvent.pointerDown(start.element, { pointerId: 1, button: 0, clientX: 244 });
    await fireEvent.pointerMove(start.element, { pointerId: 1, clientX: 264 });
    await fireEvent.pointerMove(start.element, { pointerId: 1, clientX: 400 });
    await fireEvent.pointerUp(start.element, { pointerId: 1, clientX: 400 });
    expect(start.onchange.mock.calls.map(([width]) => width)).toEqual([264, 300, 300]);

    const end = separator("end", 340);
    await fireEvent.pointerDown(end.element, { pointerId: 2, button: 0, clientX: 900 });
    await fireEvent.pointerUp(end.element, { pointerId: 2, clientX: 880 });
    expect(end.onchange).toHaveBeenLastCalledWith(360);
  });

  it("puts the width back when the system cancels a drag", async () => {
    const { element, onchange, rerender } = separator("start", 244);
    await fireEvent.pointerDown(element, { pointerId: 3, button: 0, clientX: 244 });
    await fireEvent.pointerMove(element, { pointerId: 3, clientX: 280 });
    expect(onchange).toHaveBeenLastCalledWith(280);
    await rerender({ value: 280 });
    await fireEvent.pointerCancel(element, { pointerId: 3, clientX: 280 });
    expect(onchange).toHaveBeenLastCalledWith(244);
    // The drag is over: later moves do nothing.
    onchange.mockClear();
    await fireEvent.pointerMove(element, { pointerId: 3, clientX: 300 });
    expect(onchange).not.toHaveBeenCalled();
  });
});
