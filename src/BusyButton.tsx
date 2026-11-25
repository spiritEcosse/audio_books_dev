import type { ButtonHTMLAttributes } from "react";
import type { MediaBusy } from "./mediaCost";

interface Props extends ButtonHTMLAttributes<HTMLButtonElement> {
  /** The media action that is running right now, if any. */
  busy: MediaBusy | null;
  /** Identifies this button: it shows the spinner and the running label when `busy.key` matches. */
  tag: string;
}

export function BusyButton({ busy, tag, children, disabled, ...rest }: Props) {
  const active = busy?.key === tag;
  return (
    <button {...rest} type="button" disabled={disabled || active} aria-busy={active}>
      {active ? (
        <>
          <span className="spinner" aria-hidden="true" /> {busy.label}
        </>
      ) : (
        children
      )}
    </button>
  );
}
