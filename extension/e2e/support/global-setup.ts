import { startRun, sweepOld } from "./tmp";

export default function globalSetup(): void {
  sweepOld();
  startRun();
}
