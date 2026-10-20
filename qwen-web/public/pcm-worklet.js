// Converts the first input channel to PCM16 little-endian and posts 100 ms chunks.
class Pcm16 extends AudioWorkletProcessor {
  constructor() { super(); this.size = Math.round(sampleRate / 10); this.reset(); }
  reset() { this.buffer = new Int16Array(this.size); this.length = 0; }
  process(inputs) {
    const channel = inputs[0]?.[0];
    if (channel) for (let i = 0; i < channel.length; i++) {
      const s = Math.max(-1, Math.min(1, channel[i]));
      this.buffer[this.length++] = s < 0 ? s * 0x8000 : s * 0x7fff;
      if (this.length === this.size) { this.port.postMessage(this.buffer.buffer, [this.buffer.buffer]); this.reset(); }
    }
    return true;
  }
}
registerProcessor('pcm16', Pcm16);
