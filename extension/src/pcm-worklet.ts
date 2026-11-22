// AudioWorklet processor: mixes the input down to mono and emits fixed-size signed 16-bit
// frames. The capture context runs at 16 kHz, so no resampling happens here.

declare class AudioWorkletProcessor {
  readonly port: MessagePort;
  constructor(options?: { processorOptions?: Record<string, unknown> });
}
declare function registerProcessor(name: string, ctor: new (options?: never) => unknown): void;

class PcmFramer extends AudioWorkletProcessor {
  private readonly frameSamples: number;
  private readonly buf: Int16Array;
  private fill = 0;

  constructor(options?: { processorOptions?: { frameSamples?: number } }) {
    super();
    this.frameSamples = options?.processorOptions?.frameSamples ?? 320;
    this.buf = new Int16Array(this.frameSamples);
    this.port.onmessage = (ev: MessageEvent) => {
      if (ev.data && ev.data.type === "flush") {
        this.flush();
        this.port.postMessage({ type: "flushed" });
      }
    };
  }

  private emit(count: number): void {
    const out = new Int16Array(count);
    out.set(this.buf.subarray(0, count));
    this.port.postMessage({ type: "pcm", pcm: out.buffer }, [out.buffer]);
  }

  private flush(): void {
    if (this.fill > 0) {
      this.emit(this.fill);
      this.fill = 0;
    }
  }

  process(inputs: Float32Array[][]): boolean {
    const input = inputs[0];
    if (!input || input.length === 0) return true; // keep running while the source is silent
    const n = input[0].length;
    const channels = input.length;
    for (let i = 0; i < n; i++) {
      let sum = 0;
      for (let c = 0; c < channels; c++) sum += input[c][i];
      let s = sum / channels;
      s = s < -1 ? -1 : s > 1 ? 1 : s;
      this.buf[this.fill++] = s < 0 ? s * 0x8000 : s * 0x7fff;
      if (this.fill === this.frameSamples) {
        this.emit(this.frameSamples);
        this.fill = 0;
      }
    }
    return true;
  }
}

registerProcessor("pcm-framer", PcmFramer);

export {};
