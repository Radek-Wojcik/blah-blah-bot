import { startRun, sweepOld } from "./e2e-tmp";

export default function globalSetup() {
  sweepOld();
  startRun();
}
