import type { ButtonHTMLAttributes } from "react";
import type { MediaBusy } from "./mediaCost";

interface Props extends ButtonHTMLAttributes<HTMLButtonElement> {
  /** The media actions running right now. */
  busy: readonly MediaBusy[];
  /** Identifies this button: it shows the spinner and the running label when an operation with this key is running. */
  tag: string;
}

export function BusyButton({ busy, tag, children, disabled, ...rest }: Props) {
  const running = busy.find((op) => op.key === tag);
  const active = running !== undefined;
  return (
    <button {...rest} type="button" disabled={disabled || active} aria-busy={active}>
      {active ? (
        <>
          <span className="spinner" aria-hidden="true" /> {running.label}
        </>
      ) : (
        children
      )}
    </button>
  );
}
