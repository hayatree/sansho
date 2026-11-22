// Batched SPICE level-1 junction diode evaluation.
//
// Provenance: original CUDA transcription of Sansho's own `sansho-devices/src/diode.rs`,
// `junction.rs` and `dual.rs`. Operations follow the Rust evaluation order so that,
// with FMA contraction disabled, only the transcendental library functions differ.
//
// DT selects the precision of the derivative (Jacobian) part of each dual number.
// Values and residuals are always double precision.

#ifndef DT
#define DT double
#endif

#define P 26
#define BOLTZMANN 1.380649e-23
#define CHARGE 1.602176634e-19
#define EULER 2.718281828459045
#define SQRT2 1.4142135623730951
#define MIN_NORMAL 2.2250738585072014e-308
#define MAX_FINITE 1.7976931348623157e308

struct D1 {
    double v;
    DT d;
};

__device__ __forceinline__ D1 cst(double v) { D1 r; r.v = v; r.d = (DT)0; return r; }
__device__ __forceinline__ D1 var(double v) { D1 r; r.v = v; r.d = (DT)1; return r; }
__device__ __forceinline__ D1 chain(D1 a, double v, double dv) { D1 r; r.v = v; r.d = a.d * (DT)dv; return r; }
__device__ __forceinline__ D1 add(D1 a, D1 b) { D1 r; r.v = a.v + b.v; r.d = a.d + b.d; return r; }
__device__ __forceinline__ D1 neg(D1 a) { return chain(a, -a.v, -1.0); }
__device__ __forceinline__ D1 mul(D1 a, D1 b) {
    D1 r; r.d = a.d * (DT)b.v + (DT)a.v * b.d; r.v = a.v * b.v; return r;
}
__device__ __forceinline__ D1 dvd(D1 a, D1 b) {
    double inv = 1.0 / b.v;
    D1 r; r.v = a.v * inv; r.d = (a.d - (DT)r.v * b.d) * (DT)inv; return r;
}
__device__ __forceinline__ D1 addc(D1 a, double c) { a.v += c; return a; }
__device__ __forceinline__ D1 subc(D1 a, double c) { a.v -= c; return a; }
__device__ __forceinline__ D1 mulc(D1 a, double c) { return chain(a, a.v * c, c); }
__device__ __forceinline__ D1 divc(D1 a, double c) { return mulc(a, 1.0 / c); }
__device__ __forceinline__ D1 cdiv(double c, D1 a) { return dvd(cst(c), a); }
__device__ __forceinline__ D1 csub(double c, D1 a) { return addc(neg(a), c); }
__device__ __forceinline__ D1 dexp(D1 a) { double e = exp(a.v); return chain(a, e, e); }
__device__ __forceinline__ D1 dln(D1 a) { return chain(a, log(a.v), 1.0 / a.v); }
__device__ __forceinline__ D1 dsqrt(D1 a) { double s = sqrt(a.v); return chain(a, s, 0.5 / s); }
__device__ __forceinline__ bool normal(double x) {
    double m = fabs(x);
    return m >= MIN_NORMAL && m <= MAX_FINITE;
}
__device__ __forceinline__ D1 dpowf(D1 a, double p) {
    double r = pow(a.v, p);
    double slope;
    if (a.v > 0. && normal(a.v) && normal(r)) {
        double ratio = r / a.v;
        slope = normal(ratio) ? p * ratio : p * pow(a.v, p - 1.);
    } else {
        slope = p * pow(a.v, p - 1.);
    }
    return chain(a, r, slope);
}

__device__ __forceinline__ D1 safe_exp(D1 x) {
    if (x.v <= 80.0) return dexp(x);
    double e = exp(80.0);
    return addc(mulc(subc(x, 80.0), e), e);
}
__device__ __forceinline__ D1 ideal_current(D1 v, double is, double nvt) {
    return mulc(subc(safe_exp(divc(v, nvt)), 1.0), is);
}
__device__ __forceinline__ D1 diffusion_current(D1 v, double sat, double nvt) {
    if (v.v >= -3. * nvt) return ideal_current(v, sat, nvt);
    D1 ratio = cdiv((3. * nvt) / EULER, v);
    return mulc(addc(mul(mul(ratio, ratio), ratio), 1.), -sat);
}
__device__ __forceinline__ D1 depletion_charge(D1 v, double cj0, double vj, double m, double fc) {
    if (cj0 == 0.0) return cst(0.0);
    double vfc = fc * vj;
    if (v.v < vfc) {
        D1 arg = csub(1.0, divc(v, vj));
        if (fabs(m - 1.0) < 1e-12) return mulc(neg(dln(arg)), cj0 * vj);
        return mulc(csub(1.0, dpowf(arg, 1.0 - m)), cj0 * vj / (1.0 - m));
    }
    double q_fc = fabs(m - 1.0) < 1e-12
        ? -log(1.0 - fc) * cj0 * vj
        : cj0 * vj / (1.0 - m) * (1.0 - pow(1.0 - fc, 1.0 - m));
    double f2 = pow(1.0 - fc, 1.0 + m);
    double f3 = 1.0 - fc * (1.0 + m);
    D1 dv = subc(v, vfc);
    D1 quad = mulc(subc(mul(v, v), vfc * vfc), m / (2.0 * vj));
    return addc(mulc(add(mulc(dv, f3), quad), cj0 / f2), q_fc);
}
__device__ __forceinline__ double vcrit(double is, double nvt) {
    return nvt * log(nvt / (SQRT2 * is));
}
__device__ __forceinline__ double limit_junction(double vnew, double vold, double nvt, double vc, bool* lim) {
    if (vnew > vc && fabs(vnew - vold) > 2.0 * nvt) {
        *lim = true;
        if (vold > 0.0) {
            double arg = 1.0 + (vnew - vold) / nvt;
            return arg > 0.0 ? vold + nvt * log(arg) : vc;
        }
        return nvt * log(vnew / nvt);
    }
    *lim = false;
    return vnew;
}
__device__ __forceinline__ double volt(const double* x, long long n) { return n < 0 ? 0.0 : x[n]; }

// req:    per request [lane, group, input offset, output offset]
// rctx:   per request [gmin, temperature, limit, init_junctions]
// gtab:   per group   [first instance, instance count, G len, C len, f len, q len, 0, 0]
// dtab:   per instance [a, k, a', G offset, C offset, f offset, q offset]
// params: per lane and instance, P values (see `Diodes::batch_rows`)
// input:  per request [x (dim), state (count)]
// output: per request [G, C, f, q, state (count), noncon (count)]
extern "C" __global__ void diode_eval(
    const long long* req, const double* rctx, const long long* gtab, const long long* dtab,
    const double* params, const double* input, double* output,
    int nreq, long long instances, int dim)
{
    int r = blockIdx.y;
    if (r >= nreq) return;
    long long lane = req[4 * r], grp = req[4 * r + 1];
    const long long* gt = gtab + 8 * grp;
    long long j = (long long)blockIdx.x * blockDim.x + threadIdx.x;
    if (j >= gt[1]) return;
    long long di = gt[0] + j;
    const long long* t = dtab + 7 * di;
    const double* p = params + (lane * instances + di) * P;
    const double* x = input + req[4 * r + 2];
    const double* state = x + dim;
    double* g = output + req[4 * r + 3];
    double* c = g + gt[2];
    double* f = c + gt[3];
    double* q = f + gt[4];
    double* next = q + gt[5];
    double* noncon = next + gt[1];
    double gmin = rctx[4 * r], ctemp = rctx[4 * r + 1];
    bool limit = rctx[4 * r + 2] != 0.0, init = rctx[4 * r + 3] != 0.0;

    double is0 = p[0], n = p[1], rs = p[2], cjo = p[3], vj = p[4], m = p[5], fc = p[6], tt = p[7];
    double bv = p[8], nbv = p[9], isr = p[10], nr = p[11], ikf = p[12], jsw = p[13], ns = p[14];
    double cjp = p[15], php = p[16], mjsw = p[17], fcs = p[18], perimeter = p[19], area = p[20];
    double temp = isnan(p[21]) ? ctemp : p[21];
    bool off = p[22] != 0.0, has_bd = p[25] != 0.0;
    double knee = p[23], at_bv = p[24];

    double vt = BOLTZMANN * temp / CHARGE;
    long long ag = t[3], ac = t[4], af = t[5], aq = t[6];
    if (rs > 0.0) {
        double gr = area / rs;
        double i = gr * (volt(x, t[0]) - volt(x, t[2]));
        g[ag] = gr; g[ag + 1] = -gr; g[ag + 2] = -gr; g[ag + 3] = gr;
        f[af] = i; f[af + 1] = -i;
        ag += 4; af += 2;
    }
    double vk = volt(x, t[1]);
    double va = volt(x, t[2]);
    double nvt = n * vt;
    double is = is0 * area;
    double vc = vcrit(is, nvt);
    double vd_act = va - vk;
    double vd;
    bool nc = false;
    if (init) {
        vd = off ? 0.0 : vc;
    } else if (limit) {
        double old = state[j];
        bool lim;
        double v = limit_junction(vd_act, old, nvt, vc, &lim);
        double bvt = nbv * vt;
        if (has_bd && v < fmin(-bv + 10. * bvt, 0.)) {
            bool limited;
            double vr = limit_junction(-(v + bv), -(old + bv), bvt, vcrit(at_bv, bvt), &limited);
            v = -(vr + bv);
            lim = lim || limited;
        }
        nc = lim;
        vd = v;
    } else {
        vd = vd_act;
    }
    next[j] = vd;
    noncon[j] = nc ? 1.0 : 0.0;

    D1 v = var(vd);
    D1 id = add(diffusion_current(v, is, nvt), diffusion_current(v, jsw * perimeter, ns * vt));
    D1 generation = cst(0.);
    if (isr > 0. && vd >= -3. * nvt) {
        D1 w = csub(1., divc(v, vj));
        D1 width = dpowf(addc(mul(w, w), 0.005), m * 0.5);
        generation = mul(ideal_current(v, isr * area, nr * vt), width);
    }
    id = add(id, generation);
    if (has_bd && vd < 0. && vd <= knee) {
        id = mulc(safe_exp(divc(neg(addc(v, bv)), nbv * vt)), -at_bv);
    }
    if (ikf > 0. && id.v > 0.) {
        id = dvd(id, addc(dsqrt(divc(id, ikf * area)), 1.));
    }
    D1 qd = add(add(mulc(id, tt), depletion_charge(v, cjo * area, vj, m, fc)),
                depletion_charge(v, cjp * perimeter, php, mjsw, fcs));
    id = add(id, mulc(v, gmin));

    // Two-terminal core in (a', k): residual is the expansion from (vk + vd, vk) to (va, vk).
    double dv0 = va - (vk + vd), dv1 = vk - vk;
    double gd = (double)id.d, cd = (double)qd.d;
    double ngd = (double)(-id.d), ncd = (double)(-qd.d);
    g[ag] = gd; g[ag + 1] = ngd; g[ag + 2] = ngd; g[ag + 3] = gd;
    c[ac] = cd; c[ac + 1] = ncd; c[ac + 2] = ncd; c[ac + 3] = cd;
    f[af] = id.v + gd * dv0 + ngd * dv1;
    f[af + 1] = -id.v + ngd * dv0 + gd * dv1;
    q[aq] = qd.v + cd * dv0 + ncd * dv1;
    q[aq + 1] = -qd.v + ncd * dv0 + cd * dv1;
}
