#include "ggml.h"
#include <vector>
#include <random>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cmath>
extern "C" void research_q8_dot4(int,float *,const void *,size_t,const void *);
extern "C" void research_q8_dot4_packed(int,float *,const void *,const ggml_fp16_t *,const void *);
int main(){
 std::mt19937 rng(42);std::uniform_real_distribution<float>d(-1,1);
 for(int n:{32,1024,4096}){
  int m=1024,nb=n/32;std::vector<float>w(n*m),y(n);for(auto &v:w)v=d(rng);for(auto &v:y)v=d(rng);
  std::vector<unsigned char>qw(ggml_row_size(GGML_TYPE_Q8_0,n)*m),qy(ggml_row_size(GGML_TYPE_Q8_0,n));
  ggml_quantize_chunk(GGML_TYPE_Q8_0,w.data(),qw.data(),0,m,n,nullptr);ggml_quantize_chunk(GGML_TYPE_Q8_0,y.data(),qy.data(),0,1,n,nullptr);
  void *memory=nullptr;if(posix_memalign(&memory,64,n*m*17/16))return 1;auto *p=(unsigned char *)memory;ggml_fp16_t *s=(ggml_fp16_t *)(p+n*m);
  for(int row=0;row<m;row+=4)for(int b=0;b<nb;b++)for(int j=0;j<4;j++){
   auto *q=qw.data()+((row+j)*nb+b)*34;ggml_fp16_t h;memcpy(&h,q,2);s[row*nb+b*4+j]=h;
   for(int k=0;k<32;k++)p[row*n+b*128+j*32+k]=q[2+k]^128;
  }
  std::vector<float>a(m),b(m);double times[2];
  for(int variant=0;variant<2;variant++){
   auto start=std::chrono::steady_clock::now();for(int rep=0;rep<20;rep++)for(int row=0;row<m;row+=4){
    if(variant==0)research_q8_dot4(n,a.data()+row,qw.data()+row*nb*34,nb*34,qy.data());
    else research_q8_dot4_packed(n,b.data()+row,p+row*n,s+row*nb,qy.data());
   }
   times[variant]=std::chrono::duration<double,std::micro>(std::chrono::steady_clock::now()-start).count()/20;
  }
  float error=0;for(int i=0;i<m;i++)error=std::max(error,std::abs(a[i]-b[i]));
  printf("{\"k\":%d,\"m\":%d,\"reference_us\":%.3f,\"packed_us\":%.3f,\"max_abs\":%.9g}\n",n,m,times[0],times[1],error);free(memory);if(error!=0)return 2;
 }
}
