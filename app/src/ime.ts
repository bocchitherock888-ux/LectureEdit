/**
 * True while an input method owns the keystroke. WebKit ends the composition
 * before dispatching the Enter/Escape that commits or cancels it, so that
 * keydown reports `isComposing === false` but keeps the IME keyCode 229.
 */
export function imeActive(event: { isComposing?: boolean; keyCode?: number }) {
  return Boolean(event.isComposing) || event.keyCode === 229;
}
