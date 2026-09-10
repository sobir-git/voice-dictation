#include "ggml.h"
#include "ggml-backend.h"
#include "ggml-vulkan.h"
#include <vector>
#include <random>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <algorithm>
int main(int argc,char **argv) {
    if(argc!=2) return 1;
    auto backend=ggml_backend_vk_init(0);
    if(!backend) return 2;
    FILE *output=fopen(argv[1],"wb");
    if(!output) return 3;
    std::mt19937 rng(42);std::uniform_real_distribution<float> random(-1,1);
    std::vector<std::pair<ggml_context *, ggml_backend_buffer_t>> owned;
    struct Shape {int k,m,n;};
    for(auto shape:std::vector<Shape>{{512,512,63},{512,2048,63},{2048,512,63},{512,512,127}}) {
        const int k=shape.k,m=shape.m,n=shape.n;
        auto *ctx=ggml_init({2*1024*1024,nullptr,true});
        auto *w=ggml_new_tensor_2d(ctx,GGML_TYPE_F16,k,m);
        auto *x=ggml_new_tensor_2d(ctx,GGML_TYPE_F32,k,n);
        auto *bias=ggml_new_tensor_1d(ctx,GGML_TYPE_F32,m);
        auto *y=ggml_add(ctx,ggml_mul_mat(ctx,w,x),bias);
        auto *graph=ggml_new_graph(ctx);ggml_build_forward_expand(graph,y);
        auto buffer=ggml_backend_alloc_ctx_tensors(ctx,backend);
        if(!buffer)return 4;
        ggml_backend_buffer_set_usage(buffer,GGML_BACKEND_BUFFER_USAGE_WEIGHTS);
        std::vector<float> values(k*m),input(k*n),biases(m),result(m*n);
        for(auto &v:values)v=random(rng);
        for(auto &v:input)v=random(rng);
        for(auto &v:biases)v=random(rng);
        std::vector<unsigned char> quant(ggml_nbytes(w));
        ggml_fp32_to_fp16_row(values.data(), reinterpret_cast<ggml_fp16_t *>(quant.data()), k*m);
        ggml_backend_tensor_set(w,quant.data(),0,quant.size());
        ggml_backend_tensor_set(x,input.data(),0,input.size()*sizeof(float));
        ggml_backend_tensor_set(bias,biases.data(),0,biases.size()*sizeof(float));
        for(int i=0;i<3;i++)if(ggml_backend_graph_compute(backend,graph)!=GGML_STATUS_SUCCESS)return 5;
        std::vector<double> timings;
        for(int i=0;i<25;i++) {
            auto start=std::chrono::steady_clock::now();
            if(ggml_backend_graph_compute(backend,graph)!=GGML_STATUS_SUCCESS)return 6;
            ggml_backend_synchronize(backend);
            timings.push_back(std::chrono::duration<double,std::micro>(std::chrono::steady_clock::now()-start).count());
        }
        std::sort(timings.begin(),timings.end());
        ggml_backend_tensor_get(y,result.data(),0,m*n*sizeof(float));
        fwrite(result.data(),sizeof(float),m*n,output);
        printf("{\"k\":%d,\"m\":%d,\"n\":%d,\"median_us\":%.3f,\"p90_us\":%.3f}\n",k,m,n,timings[12],timings[22]);fflush(stdout);
        // Keep tensor addresses unique while the experimental weight cache is alive.
        owned.emplace_back(ctx,buffer);
    }
    fclose(output);
    ggml_backend_free(backend);
    for(auto [ctx,buffer]:owned){ggml_backend_buffer_free(buffer);ggml_free(ctx);}
    return 0;
}
