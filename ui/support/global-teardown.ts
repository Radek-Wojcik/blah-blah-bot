import { endRun } from "./e2e-tmp";

export default function globalTeardown() {
  endRun();
}
