// flux-gpu — CUDA BLAKE3 miner kernel (the SIGIL "BLAKE4" PoW on the GPU).
// PoW: blake3(header || nonce_le8) → low 8 bytes as LE u64 ≤ target.
// Honesty: self-tests against the official BLAKE3 empty-input vector
// (af1349b9 f5f9a1a6 ...) on the GPU BEFORE measuring any hashrate.
// Build (on a CUDA box): nvcc -O3 -arch=native blake3_miner.cu -o blake3_miner
#include <cstdint>
#include <cstdio>
#include <cuda_runtime.h>

__device__ __forceinline__ uint32_t rotr(uint32_t x, int n){ return (x>>n)|(x<<(32-n)); }

__device__ __constant__ uint32_t IV[8] = {
  0x6A09E667u,0xBB67AE85u,0x3C6EF372u,0xA54FF53Au,0x510E527Fu,0x9B05688Cu,0x1F83D9ABu,0x5BE0CD19u };

__device__ __forceinline__ void g(uint32_t* s,int a,int b,int c,int d,uint32_t mx,uint32_t my){
  s[a]=s[a]+s[b]+mx; s[d]=rotr(s[d]^s[a],16);
  s[c]=s[c]+s[d];    s[b]=rotr(s[b]^s[c],12);
  s[a]=s[a]+s[b]+my; s[d]=rotr(s[d]^s[a],8);
  s[c]=s[c]+s[d];    s[b]=rotr(s[b]^s[c],7);
}
__device__ __forceinline__ void round_fn(uint32_t* s, const uint32_t* m){
  g(s,0,4,8,12, m[0], m[1]);  g(s,1,5,9,13, m[2], m[3]);
  g(s,2,6,10,14,m[4], m[5]);  g(s,3,7,11,15,m[6], m[7]);
  g(s,0,5,10,15,m[8], m[9]);  g(s,1,6,11,12,m[10],m[11]);
  g(s,2,7,8,13, m[12],m[13]); g(s,3,4,9,14, m[14],m[15]);
}
// single-block (input <=64B) BLAKE3 compression, ROOT output; returns out[8] words.
__device__ void blake3_oneblock(const uint32_t* block, uint32_t block_len, uint32_t* out){
  const uint32_t FLAGS = 1u|2u|8u; // CHUNK_START|CHUNK_END|ROOT
  uint32_t s[16];
  #pragma unroll
  for(int i=0;i<8;i++) s[i]=IV[i];
  s[8]=IV[0];s[9]=IV[1];s[10]=IV[2];s[11]=IV[3];
  s[12]=0;s[13]=0;s[14]=block_len;s[15]=FLAGS;
  uint32_t m[16];
  #pragma unroll
  for(int i=0;i<16;i++) m[i]=block[i];
  const int P[16]={2,6,3,10,7,0,4,13,1,11,12,5,9,14,15,8};
  for(int r=0;r<7;r++){
    round_fn(s,m);
    if(r<6){ uint32_t pm[16];
      #pragma unroll
      for(int i=0;i<16;i++) pm[i]=m[P[i]];
      #pragma unroll
      for(int i=0;i<16;i++) m[i]=pm[i]; }
  }
  #pragma unroll
  for(int i=0;i<8;i++) out[i]=s[i]^s[i+8];
}

// ── self-test: blake3("") must equal the official empty-input vector ──
__global__ void selftest(uint32_t* out){
  uint32_t block[16]; for(int i=0;i<16;i++) block[i]=0;
  blake3_oneblock(block, 0, out); // block_len 0 = empty input
}

// ── miner: each thread tries nonce = base+tid, header is `hlen` bytes ──
__global__ void mine(const uint8_t* header, uint32_t hlen, uint64_t base,
                     uint64_t target, unsigned long long* hits, uint64_t* found_nonce){
  uint64_t nonce = base + (uint64_t)(blockIdx.x)*blockDim.x + threadIdx.x;
  uint8_t buf[64];
  #pragma unroll
  for(int i=0;i<64;i++) buf[i]=0;
  for(uint32_t i=0;i<hlen && i<56;i++) buf[i]=header[i];
  // nonce LE at offset hlen
  for(int i=0;i<8;i++) buf[hlen+i]=(uint8_t)(nonce>>(8*i));
  uint32_t block[16];
  #pragma unroll
  for(int i=0;i<16;i++)
    block[i]=(uint32_t)buf[4*i]|((uint32_t)buf[4*i+1]<<8)|((uint32_t)buf[4*i+2]<<16)|((uint32_t)buf[4*i+3]<<24);
  uint32_t out[8];
  blake3_oneblock(block, hlen+8, out);
  uint64_t low = (uint64_t)out[0] | ((uint64_t)out[1]<<32); // low 8 bytes LE
  atomicAdd(hits, 1ULL);
  if(low <= target){ *found_nonce = nonce; }
}

int main(){
  // 1) SELF-TEST against the official BLAKE3 empty vector.
  uint32_t *d_out; cudaMalloc(&d_out, 32);
  selftest<<<1,1>>>(d_out);
  uint32_t h_out[8]; cudaMemcpy(h_out, d_out, 32, cudaMemcpyDeviceToHost);
  // expected: af1349b9 f5f9a1a6 a0404dea 36dcc949 9bcb25c9 adc112b7 cc9a93ca e41f3262 (LE words)
  const uint32_t exp[8]={0xb94913afu,0xa6a1f9f5u,0xea4d40a0u,0x49c9dc36u,0xc925cb9bu,0xb712c1adu,0xca939accu,0x62321fe4u};
  bool ok=true; for(int i=0;i<8;i++) if(h_out[i]!=exp[i]) ok=false;
  printf("BLAKE3 self-test (empty input): %s\n", ok?"PASS ✓ (matches official af1349b9...)":"FAIL ✗");
  printf("  got: "); for(int i=0;i<8;i++) printf("%08x", __builtin_bswap32(h_out[i])); printf("\n");
  if(!ok){ printf("ABORT — kernel is not real BLAKE3, GH/s would be meaningless\n"); return 1; }

  // 2) MEASURE hashrate.
  const char* hdr = "sigil-g0-demo-block-header"; uint32_t hlen=26;
  uint8_t* d_hdr; cudaMalloc(&d_hdr,hlen); cudaMemcpy(d_hdr,hdr,hlen,cudaMemcpyHostToDevice);
  unsigned long long* d_hits; cudaMalloc(&d_hits,8); cudaMemset(d_hits,0,8);
  uint64_t* d_found; cudaMalloc(&d_found,8); cudaMemset(d_found,0,8);
  uint64_t target=0x00003fffffffffffULL; // same as the CPU miner demo
  int threads=256, blocks=65535; uint64_t per_launch=(uint64_t)threads*blocks;
  cudaEvent_t a,b; cudaEventCreate(&a); cudaEventCreate(&b);
  cudaEventRecord(a);
  int launches=40;
  for(int l=0;l<launches;l++) mine<<<blocks,threads>>>(d_hdr,hlen,(uint64_t)l*per_launch,target,d_hits,d_found);
  cudaEventRecord(b); cudaEventSynchronize(b);
  float ms=0; cudaEventElapsedTime(&ms,a,b);
  unsigned long long hits; cudaMemcpy(&hits,d_hits,8,cudaMemcpyDeviceToHost);
  double ghs = (double)hits/ (ms/1000.0) / 1e9;
  printf("MINED %llu blake3 hashes in %.1f ms → %.3f GH/s\n",(unsigned long long)hits, ms, ghs);
  printf("  (CPU flux-miner reference: ~0.256 GH/s on 48 cores → GPU speedup ~%.0fx)\n", ghs/0.256);
  return 0;
}
