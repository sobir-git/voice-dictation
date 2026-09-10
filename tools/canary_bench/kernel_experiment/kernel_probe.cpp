#include "ggml.h"
#include "ggml-cpu.h"
#include "quants.h"
#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <random>
#include <vector>
extern "C" void research_q8_dot4(int, float *, const void *, size_t, const void *);
using Clock = std::chrono::steady_clock;
int main() {
    ggml_cpu_init();
    std::mt19937 rng(42);
    std::normal_distribution<float> dist(0, 1);
    for (int k : {512, 1024, 4096}) {
        const int rows=1024;
        const size_t stride=ggml_row_size(GGML_TYPE_Q8_0,k);
        std::vector<float> floats(k*rows), input(k), ref(rows), actual(rows);
        for (auto &x:floats) x=dist(rng);
        for (auto &x:input) x=dist(rng);
        std::vector<unsigned char> weights(rows*stride), qinput(stride);
        for(int r=0;r<rows;r++) quantize_row_q8_0(floats.data()+r*k,weights.data()+r*stride,k);
        quantize_row_q8_0(input.data(),qinput.data(),k);
        auto run=[&](bool fused){
            if(fused) for(int r=0;r<rows;r+=4) research_q8_dot4(k,actual.data()+r,weights.data()+r*stride,stride,qinput.data());
            else for(int r=0;r<rows;r++) ggml_vec_dot_q8_0_q8_0(k,ref.data()+r,0,weights.data()+r*stride,0,qinput.data(),0,1);
        };
        run(false);run(true);
        float maxerror=0;
        for(int r=0;r<rows;r++) maxerror=std::max(maxerror,std::abs(ref[r]-actual[r]));
        printf("q8-check k=%d max_abs=%.9g bit_equal=%d\n",k,maxerror,memcmp(ref.data(),actual.data(),rows*sizeof(float))==0);
        if(maxerror!=0) return 2;
        for(int round=0;round<6;round++) {
            bool fused=round%2;
            auto start=Clock::now();
            for(int i=0;i<150;i++) run(fused);
            double us=std::chrono::duration<double,std::micro>(Clock::now()-start).count()/150;
            printf("q8-time k=%d variant=%s round=%d us=%.3f\n",k,fused?"four-row":"reference",round,us);
        }
    }
    // Full graph exercises fusion eligibility and tests against the original unfused sequence.
    // Execute in separate processes with CANARY_FUSE_LN unset/set; compare printed checksums/files.
    FILE * output=fopen(std::getenv("CANARY_FUSE_LN")?"ln-fused.bin":"ln-reference.bin","wb");
    for(int width : {512,1024}) for(int rows : {1,9,71,318}) {
        ggml_init_params init={64*1024*1024,nullptr,false};
        auto * ctx=ggml_init(init);
        auto * x=ggml_new_tensor_2d(ctx,GGML_TYPE_F32,width,rows);
        auto * g=ggml_new_tensor_1d(ctx,GGML_TYPE_F32,width);
        auto * b=ggml_new_tensor_1d(ctx,GGML_TYPE_F32,width);
        for(int i=0;i<width*rows;i++) ((float*)x->data)[i]=dist(rng);
        for(int i=0;i<width;i++){((float*)g->data)[i]=dist(rng);((float*)b->data)[i]=dist(rng);}
        auto * out=ggml_add(ctx,ggml_mul(ctx,ggml_norm(ctx,x,1e-5f),g),b);
        auto * graph=ggml_new_graph(ctx);ggml_build_forward_expand(graph,out);
        auto plan=ggml_graph_plan(graph,8,nullptr);
        std::vector<unsigned char> work(plan.work_size);plan.work_data=work.data();
        for(int i=0;i<3;i++) if(ggml_graph_compute(graph,&plan)!=GGML_STATUS_SUCCESS) return 3;
        auto start=Clock::now();
        for(int i=0;i<100;i++) ggml_graph_compute(graph,&plan);
        printf("ln-time width=%d rows=%d us=%.3f\n",width,rows,std::chrono::duration<double,std::micro>(Clock::now()-start).count()/100);
        fwrite(out->data,sizeof(float),width*rows,output);
        ggml_free(ctx);
    }
    fclose(output);
    FILE * gemv_file=fopen(std::getenv("CANARY_GEMV_BIAS")?"gemv-fused.bin":"gemv-reference.bin","wb");
    for(int k : {512,1024,4096}) for(int m : {32,512,1024}) {
        ggml_init_params init={64*1024*1024,nullptr,false};
        auto * ctx=ggml_init(init);
        auto * w=ggml_new_tensor_2d(ctx,GGML_TYPE_Q8_0,k,m);
        auto * x=ggml_new_tensor_1d(ctx,GGML_TYPE_F32,k);
        auto * b=ggml_new_tensor_1d(ctx,GGML_TYPE_F32,m);
        std::vector<float> row(k);
        for(int r=0;r<m;r++) {
            for(auto &v:row)v=dist(rng);
            quantize_row_q8_0(row.data(),(char*)w->data+r*w->nb[1],k);
        }
        for(int i=0;i<k;i++) ((float*)x->data)[i]=dist(rng);
        for(int i=0;i<m;i++) ((float*)b->data)[i]=dist(rng);
        auto * out=ggml_add(ctx,ggml_mul_mat(ctx,w,x),b);
        auto * graph=ggml_new_graph(ctx);ggml_build_forward_expand(graph,out);
        for(int threads : {1,2,8}) {
            auto plan=ggml_graph_plan(graph,threads,nullptr);
            std::vector<unsigned char> work(plan.work_size);plan.work_data=work.data();
            for(int i=0;i<3;i++) if(ggml_graph_compute(graph,&plan)!=GGML_STATUS_SUCCESS) return 3;
            auto start=Clock::now();
            for(int i=0;i<100;i++) ggml_graph_compute(graph,&plan);
            printf("gemv-time k=%d m=%d threads=%d us=%.3f\n",k,m,threads,std::chrono::duration<double,std::micro>(Clock::now()-start).count()/100);
            fwrite(out->data,sizeof(float),m,gemv_file);
        }
        ggml_free(ctx);
    }
    fclose(gemv_file);
}
