import { useEffect, useRef } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";

const EXIT_EVENT = "pty-exit";
const EXIT_NOTICE = "\r\n\x1b[2m[сессия завершена — закройте и откройте консоль заново]\x1b[0m\r\n";

interface Props {
  visible: boolean;
}

/** Embedded Claude Code terminal. Stays mounted while hidden so the session survives. */
export function ClaudeConsole({ visible }: Props) {
  const hostRef = useRef<HTMLDivElement>(null);
  const fitRef = useRef<FitAddon | null>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    const term = new Terminal({ fontSize: 13, cursorBlink: true, convertEol: false });
    const fit = new FitAddon();
    fitRef.current = fit;
    term.loadAddon(fit);
    term.open(host);
    fit.fit();

    const output = new Channel<ArrayBuffer>();
    output.onmessage = (chunk) => term.write(new Uint8Array(chunk));

    // pty_spawn replaces any previous session, so no explicit kill is needed on cleanup.
    invoke("pty_spawn", { rows: term.rows, cols: term.cols, output }).catch(
      (err) => term.write(`\r\n\x1b[31m${String(err)}\x1b[0m\r\n`),
    );

    const input = term.onData((data) => void invoke("pty_write", { data }).catch(() => {}));
    const resize = term.onResize(({ rows, cols }) =>
      void invoke("pty_resize", { rows, cols }).catch(() => {}),
    );
    const observer = new ResizeObserver(() => {
      if (host.offsetParent !== null) fit.fit();
    });
    observer.observe(host);
    const unlistenExit = listen(EXIT_EVENT, () => term.write(EXIT_NOTICE));

    return () => {
      observer.disconnect();
      input.dispose();
      resize.dispose();
      void unlistenExit.then((off) => off());
      term.dispose();
    };
  }, []);

  useEffect(() => {
    if (visible) fitRef.current?.fit();
  }, [visible]);

  return <div className="console" style={{ display: visible ? "block" : "none" }} ref={hostRef} />;
}
