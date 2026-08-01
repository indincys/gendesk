import { act, renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { useListSelection } from "./useListSelection";

function keyEvent(key: string, modifiers: { metaKey?: boolean; ctrlKey?: boolean } = {}) {
  return {
    key,
    metaKey: false,
    ctrlKey: false,
    shiftKey: false,
    target: document.createElement("div"),
    preventDefault: vi.fn(),
    ...modifiers,
  } as unknown as React.KeyboardEvent;
}

describe("useListSelection", () => {
  it("supports range, command toggle and command-A selection", () => {
    const { result } = renderHook(() => useListSelection([1, 2, 3, 4]));
    act(() => result.current.select(2));
    act(() => result.current.select(4, { shiftKey: true, metaKey: false, ctrlKey: false }));
    expect([...result.current.selected]).toEqual([2, 3, 4]);

    act(() => result.current.select(1, { shiftKey: false, metaKey: true, ctrlKey: false }));
    expect([...result.current.selected]).toEqual([2, 3, 4, 1]);

    act(() => result.current.containerProps.onKeyDown(keyEvent("a", { metaKey: true })));
    expect([...result.current.selected]).toEqual([1, 2, 3, 4]);
  });

  it("routes Enter/Delete/Escape and arrow focus through the shared actions", () => {
    const onOpen = vi.fn();
    const onDelete = vi.fn();
    const { result } = renderHook(() => useListSelection([10, 20, 30], { onOpen, onDelete }));

    act(() => result.current.containerProps.onKeyDown(keyEvent("ArrowDown")));
    expect([...result.current.selected]).toEqual([10]);
    act(() => result.current.containerProps.onKeyDown(keyEvent("ArrowDown")));
    expect([...result.current.selected]).toEqual([20]);
    act(() => result.current.containerProps.onKeyDown(keyEvent("Enter")));
    expect(onOpen).toHaveBeenCalledWith(20);
    act(() => result.current.containerProps.onKeyDown(keyEvent("Delete")));
    expect(onDelete).toHaveBeenCalledWith([20]);
    act(() => result.current.containerProps.onKeyDown(keyEvent("Escape")));
    expect(result.current.selected.size).toBe(0);
  });
});
