import { showComparison } from "./util/compare"

const ARRAY_LENGTH = 2

const arrbuf = {
  int8arr: new Int8Array(ARRAY_LENGTH),
  uint8arr: new Uint8Array(ARRAY_LENGTH),
  uint8clamp: new Uint8ClampedArray(ARRAY_LENGTH),
  int16arr: new Int16Array(ARRAY_LENGTH),
  uint16arr: new Uint16Array(ARRAY_LENGTH),
  int32arr: new Int32Array(ARRAY_LENGTH),
  uint32arr: new Uint32Array(ARRAY_LENGTH),
  float32arr: new Float32Array(ARRAY_LENGTH),
  float64arr: new Float64Array(ARRAY_LENGTH),
  bigint64arr: new BigInt64Array(ARRAY_LENGTH),
  biguint64arr: new BigUint64Array(ARRAY_LENGTH),
  dataview: new DataView(new ArrayBuffer(ARRAY_LENGTH)),
  arrbuf: new ArrayBuffer(ARRAY_LENGTH),
  shaarrbuf: new SharedArrayBuffer(ARRAY_LENGTH),
  buffer: Buffer.alloc(ARRAY_LENGTH),
  longbuf: Buffer.alloc(100),
}

showComparison(arrbuf, "Array Buffers (and related objects)")
