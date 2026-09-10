// Tiny CUDA workload for verifying cuprof end to end.
// Produces: HtoD memcpy, memset, kernel launches on two streams, DtoH memcpy.
//
//   make examples && ./build/cuprof run -v -o /tmp/t.json -- ./build/vector_add

#include <cstdio>

__global__ void VectorAdd(const float* a, const float* b, float* c, int n) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i < n) c[i] = a[i] + b[i];
}

__global__ void Scale(float* c, float k, int n) {
    int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i < n) c[i] *= k;
}

int main() {
    const int n = 1 << 20;
    const size_t bytes = n * sizeof(float);

    float* h_a = new float[n];
    float* h_c = new float[n];
    for (int i = 0; i < n; ++i) h_a[i] = 1.0f;

    float *d_a, *d_b, *d_c;
    cudaMalloc(&d_a, bytes);
    cudaMalloc(&d_b, bytes);
    cudaMalloc(&d_c, bytes);

    cudaStream_t s1, s2;
    cudaStreamCreate(&s1);
    cudaStreamCreate(&s2);

    cudaMemcpyAsync(d_a, h_a, bytes, cudaMemcpyHostToDevice, s1);
    cudaMemsetAsync(d_b, 0, bytes, s2);

    const int threads = 256;
    const int blocks = (n + threads - 1) / threads;
    for (int iter = 0; iter < 5; ++iter) {
        VectorAdd<<<blocks, threads, 0, s1>>>(d_a, d_b, d_c, n);
        Scale<<<blocks, threads, 0, s2>>>(d_c, 1.5f, n);
    }

    cudaMemcpyAsync(h_c, d_c, bytes, cudaMemcpyDeviceToHost, s1);
    cudaStreamSynchronize(s1);
    cudaStreamSynchronize(s2);

    printf("c[0] = %f\n", h_c[0]);

    cudaStreamDestroy(s1);
    cudaStreamDestroy(s2);
    cudaFree(d_a);
    cudaFree(d_b);
    cudaFree(d_c);
    delete[] h_a;
    delete[] h_c;
    return 0;
}
