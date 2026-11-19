import { spawn } from "node:child_process";
import { createWriteStream } from "node:fs";
/** Run without a shell, stream diagnostics, and bound both exit and cleanup. */
export function command(
  args,
  { cwd, env = process.env, log, timeoutMs = 600000 } = {},
) {
  return new Promise((resolve, reject) => {
    const output = log ? createWriteStream(log, { flags: "a" }) : undefined;
    const child = spawn(args[0], args.slice(1), {
      cwd,
      env,
      detached: process.platform !== "win32",
      stdio: ["ignore", "pipe", "pipe"],
    });
    for (const [stream, destination] of [
      [child.stdout, process.stdout],
      [child.stderr, process.stderr],
    ])
      stream.on("data", (data) => {
        destination.write(data);
        output?.write(data);
      });
    let timedOut = false;
    let finished = false;
    let force;
    const signal = (name) => {
      if (!child.pid) return;
      try {
        if (process.platform === "win32") child.kill(name);
        else process.kill(-child.pid, name);
      } catch (error) {
        if (error.code !== "ESRCH") throw error;
      }
    };
    const interrupt = () => {
      signal("SIGTERM");
      force = setTimeout(() => signal("SIGKILL"), 5000);
    };
    process.once("SIGINT", interrupt);
    process.once("SIGTERM", interrupt);
    const timer = setTimeout(() => {
      timedOut = true;
      interrupt();
    }, timeoutMs);
    const finish = (error) => {
      if (finished) return;
      finished = true;
      clearTimeout(timer);
      clearTimeout(force);
      process.removeListener("SIGINT", interrupt);
      process.removeListener("SIGTERM", interrupt);
      if (error) signal("SIGKILL");
      output?.end();
      error ? reject(error) : resolve();
    };
    child.on("error", finish);
    child.on("exit", (code, signal) =>
      finish(
        code === 0 && !timedOut
          ? undefined
          : new Error(
              `${args.join(" ")} ${timedOut ? "timed out" : `exited ${code ?? signal}`}`,
            ),
      ),
    );
  });
}
export async function stopProcess(child) {
  if (!child?.pid || child.exitCode !== null || child.signalCode !== null)
    return;
  await new Promise((resolve) => {
    const timer = setTimeout(() => child.kill("SIGKILL"), 5000);
    child.once("exit", () => {
      clearTimeout(timer);
      resolve();
    });
    child.kill("SIGTERM");
  });
}
