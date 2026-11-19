//! Authored compiler/evaluator regression models.
//! Provenance: original test equations, Accellera LRM 2.4, and elementary circuit theory.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use sansho_va::*;
fn compile(text: &str) -> CompiledModule {
    compile_module(&parse_va(text, &NoVaFiles).unwrap(), None).unwrap()
}
fn run(m: &CompiledModule, b: &Bound, x: &[f64]) -> EvalOutput {
    let mut state = vec![0.; m.state_len()];
    let accepted = state.clone();
    let mut out = EvalOutput::default();
    m.eval(
        b,
        &EvalInput {
            x,
            temperature: 300.,
            analysis: Analysis::Dc,
            initial_step: false,
            final_step: false,
            abstime: 0.,
            gmin: 1e-12,
            limit: false,
            simparams: &|_| None,
            mfactor: 1.,
        },
        StateIo {
            accepted: &accepted,
            current: &mut state,
        },
        &mut out,
    );
    assert!(out.error.is_none(), "{:?}", out.error);
    out
}
fn close(a: f64, b: f64) {
    assert!(
        (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1e-20),
        "{a:e} != {b:e}"
    );
}
fn finite_difference(m: &CompiledModule, b: &Bound, x: &[f64]) {
    let out = run(m, b, x);
    for col in 0..x.len() {
        let h = 1e-6 * x[col].abs().max(0.01);
        let mut hi = x.to_vec();
        let mut lo = hi.clone();
        hi[col] += h;
        lo[col] -= h;
        let hi = run(m, b, &hi);
        let lo = run(m, b, &lo);
        for row in 0..x.len() {
            close(
                out.df[row * x.len() + col],
                (hi.f[row] - lo.f[row]) / (2. * h),
            );
            close(
                out.dq[row * x.len() + col],
                (hi.q[row] - lo.q[row]) / (2. * h),
            );
        }
    }
}
#[test]
fn resistor_and_bindings() {
    let m = compile(include_str!("models/resistor.va"));
    assert_eq!(m.ports, ["p", "n"]);
    let b = m.bind(&[]).unwrap();
    let o = run(&m, &b, &[2., 0.5]);
    assert_eq!(o.f, [0.0015, -0.0015]);
    assert_eq!(o.df, [0.001, -0.001, -0.001, 0.001]);
    assert_eq!(o.q, [0., 0.]);
    assert!(
        m.bind(&[("r".into(), 0.)])
            .unwrap_err()
            .iter()
            .all(|d| d.code == "E-VA-PARAM")
    );
    assert!(m.bind(&[("unknown".into(), 1.)]).is_err());
    let model = m.bind(&[("r".into(), 100.)]).unwrap();
    let instance = m.bind_instance(&model, &[("r".into(), 200.)]).unwrap();
    assert_eq!(run(&m, &instance, &[1., 0.]).f[0], 0.005);
}
#[test]
fn diode_charge_series_and_limit() {
    let m = compile(
        r#"
module junction(a,k); inout a,k; electrical a,k,j;
parameter real isat=1e-14 from (0:inf), rs=2 from [0:inf), vt=0.02585;
parameter real cj=1e-12 from [0:inf), tau=1e-9 from [0:inf);
aliasparam saturation=isat; real v, ids, charge; branch (a,j) series;
analog function real exponential; input x; real x; begin exponential=exp(x); end endfunction
analog begin
V(series) <+ rs*I(series);
v=$limit(V(j,k),"pnjlim",vt,0.6);
ids=isat*(exponential(v/vt)-1);
charge=cj*v+tau*ids;
I(j,k) <+ ids+ddt(charge);
end endmodule
"#,
    );
    assert_eq!(m.internal_nodes, ["j"]);
    assert_eq!(m.branch_unknowns, ["series"]);
    assert_eq!(m.state_len(), 1);
    let b = m.bind(&[]).unwrap();
    finite_difference(&m, &b, &[0.5, 0.01, 0.48, 1e-5]);
    let o = run(&m, &b, &[0.5, 0., 0.48, 1e-5]);
    close(o.f[3], 0.02 - 2e-5);
    assert_eq!(o.df[3 * 4], 1.);
    assert_eq!(o.df[3 * 4 + 2], -1.);
    assert_eq!(o.df[3 * 4 + 3], -2.);
    let mut current = [0.];
    let accepted = [0.];
    let mut out = EvalOutput::default();
    let input = EvalInput {
        x: &[2., 0., 2., 0.],
        temperature: 300.,
        analysis: Analysis::Dc,
        initial_step: false,
        final_step: false,
        abstime: 0.,
        gmin: 0.,
        limit: true,
        simparams: &|_| None,
        mfactor: 1.,
    };
    m.eval(
        &b,
        &input,
        StateIo {
            accepted: &accepted,
            current: &mut current,
        },
        &mut out,
    );
    assert!(out.noncon);
    assert!(current[0] > 0. && current[0] < 2.);
    let previous = current[0];
    m.eval(
        &b,
        &input,
        StateIo {
            accepted: &accepted,
            current: &mut current,
        },
        &mut out,
    );
    assert!(current[0] > previous);
    let input = EvalInput {
        limit: false,
        ..input
    };
    m.eval(
        &b,
        &input,
        StateIo {
            accepted: &accepted,
            current: &mut current,
        },
        &mut out,
    );
    assert!(!out.noncon);
    assert_eq!(current[0], 2.);
    assert!(m.bind(&[("saturation".into(), -1.)]).is_err());
}
#[test]
fn nonlinear_mos_charge() {
    let m = compile(
        r#"module mos(d,g,s); electrical d,g,s; parameter real beta=0.001, c=1e-12; real over, ids, charge;
analog begin over=max(V(g,s)-0.4,0); ids=beta*over*over*tanh(V(d,s)); charge=c*over*over*(1+0.1*V(d,s)); I(d,s) <+ ids+ddt(charge); I(g,s) <+ ddt(c*V(g,s)*V(g,s)); end endmodule"#,
    );
    let b = m.bind(&[]).unwrap();
    finite_difference(&m, &b, &[0.7, 1.1, 0.1]);
}
#[test]
fn language_semantics() {
    let m = compile(
        r#"module semantics(p); electrical p;
parameter integer mode=0 from [0:3] exclude 2; parameter real scale=2; parameter string tag="go"; aliasparam gain=scale;
integer truncation, k; real total, array[2:4], outval;
analog function real twice; input x; output y; real x,y; begin y=2*x; twice=y+1; end endfunction
analog begin : scope
real temp;
truncation=7.9; total=(7/2)+(7.0/2)+truncation;
for(k=2;k<=4;k=k+1) array[k]=k;
case(mode) 0,1: total=total+array[2]+array[4]; default: total=0; endcase
if (tag=="go") total=total+1;
temp=twice(3,outval); total=total+temp+outval;
repeat(2) total=total+1;
k=0; while(k<2) begin total=total+1; k=k+1; end
if (0 && (1/0)) total=0; if (1 || (1/0)) total=total+1;
if (analysis("dc")) total=total+$simparam("missing",5);
if ($param_given(scale)) total=total+scale;
if ($port_connected(p)) total=total+1;
I(p) <+ total;
end endmodule"#,
    );
    let b = m.bind(&[]).unwrap();
    close(run(&m, &b, &[0.]).f[0], 44.5);
    let b = m.bind(&[("gain".into(), 3.)]).unwrap();
    close(run(&m, &b, &[0.]).f[0], 47.5);
    assert!(m.bind(&[("mode".into(), 1.5)]).is_err());
}
#[test]
fn voltage_sum_ground_and_reactive_sign() {
    let m = compile(
        r#"module source(p,n); electrical p,n; ground n; parameter real r=2; analog begin V(p,n)<+r*I(p,n); V(p,n)<+1+ddt(3*V(p)); end endmodule"#,
    );
    assert_eq!(m.unknown_len(), 3);
    let b = m.bind(&[]).unwrap();
    let out = run(&m, &b, &[4., 100., 0.5]);
    assert_eq!(out.f, [0.5, 0., 2.]);
    assert_eq!(out.q, [0., 0., -12.]);
    assert_eq!(out.dq[6], -3.);
    finite_difference(&m, &b, &[4., 100., 0.5]);
}
#[test]
fn ddx_and_builtins() {
    let m = compile(
        r#"module math(p); electrical p; real x;
analog begin x=V(p); I(p)<+exp(x)+ln(x)+log(x)+sqrt(x)+pow(x,3)+sin(x)+cos(x)+tan(x)+asin(x)+acos(x)+atan(x)+sinh(x)+cosh(x)+tanh(x)+asinh(x)+acosh(x+1)+atanh(x)+hypot(x,2)+atan2(x,2)+abs(x)+min(x,3)+max(x,0)+floor(x)+ceil(x); end endmodule"#,
    );
    let b = m.bind(&[]).unwrap();
    finite_difference(&m, &b, &[0.3]);
    let m =
        compile("module derivative(p); electrical p; analog I(p)<+ddx(V(p)*V(p),V(p)); endmodule");
    let b = m.bind(&[]).unwrap();
    let out = run(&m, &b, &[3.]);
    assert_eq!(out.f, [6.]);
    assert_eq!(out.df, [0.]);
}
#[test]
fn initialization_and_runtime_errors() {
    let m = compile(
        r#"module initial_events(p); electrical p; real v; analog begin @(initial_step) v=4; I(p)<+v; end endmodule"#,
    );
    let b = m.bind(&[]).unwrap();
    let mut o = EvalOutput::default();
    let input = EvalInput {
        x: &[0.],
        temperature: 300.,
        analysis: Analysis::Dc,
        initial_step: true,
        final_step: false,
        abstime: 0.,
        gmin: 0.,
        limit: false,
        simparams: &|_| None,
        mfactor: 1.,
    };
    m.eval(
        &b,
        &input,
        StateIo {
            accepted: &[],
            current: &mut [],
        },
        &mut o,
    );
    assert_eq!(o.f, [4.]);
    assert_eq!(run(&m, &b, &[0.]).f, [4.]);
    for source in [
        r#"module m(p); electrical p; analog $fatal("bad"); endmodule"#,
        r#"module m(p); electrical p; analog while(1) ; endmodule"#,
        r#"module m(p); electrical p; real a[0:1]; analog a[2]=1; endmodule"#,
    ] {
        let m = compile(source);
        let b = m.bind(&[]).unwrap();
        m.eval(
            &b,
            &input,
            StateIo {
                accepted: &[],
                current: &mut [],
            },
            &mut o,
        );
        assert!(o.error.is_some());
    }
}
#[test]
fn derivative_states_and_product_jacobian() {
    let m = compile(
        "module m(p); electrical p; real u; analog begin u=ddt(V(p)*V(p)); I(p)<+V(p)*u+sin(ddt(V(p))); end endmodule",
    );
    let b = m.bind(&[]).unwrap();
    let x = [2., 3., 0.25];
    let out = run(&m, &b, &x);
    close(out.f[0], 6. + 0.25_f64.sin());
    assert_eq!(&out.f[1..], &[3., 0.25]);
    assert_eq!(out.q, [0., -4., -2.]);
    // The first partial is c' * qdot. Eliminating the two unit-diagonal
    // operator rows adds alpha * (2*p*p + cos(qdot2)).
    assert_eq!(out.df[0], 3.);
    assert_eq!(out.df[1], 2.);
    close(out.df[2], 0.25_f64.cos());
    assert_eq!(out.dq[3], -4.);
    assert_eq!(out.dq[6], -1.);
    finite_difference(&m, &b, &x);

    let m = compile("module m(p); electrical p; analog I(p)<+ddt(ddt(V(p))); endmodule");
    let b = m.bind(&[]).unwrap();
    let out = run(&m, &b, &[2., 3.]);
    assert_eq!(out.f, [0., 3.]);
    assert_eq!(out.q, [3., -2.]);
}

#[test]
fn genvar_derivatives_have_binding_specific_rows() {
    let m = compile(
        "module m(p); electrical p; parameter integer n=2; genvar i,j;
        analog for(i=1;i<=n;i=i+1) for(j=0;j<i;j=j+1)
            I(p)<+V(p)*ddt((i+j)*V(p)); endmodule",
    );
    for n in [0, 2, 3] {
        let b = m.bind(&[("n".into(), n as f64)]).unwrap();
        let weights: Vec<_> = (1..=n)
            .flat_map(|i| (0..i).map(move |j| f64::from(i + j)))
            .collect();
        let mut x = vec![2.; 1 + weights.len().max(1)];
        for (i, state) in x[1..].iter_mut().enumerate() {
            *state = i as f64 + 1.;
        }
        for _ in 0..2 {
            let out = run(&m, &b, &x);
            assert_eq!(out.f[0], 2. * x[1..1 + weights.len()].iter().sum::<f64>());
            for (i, gain) in weights.iter().enumerate() {
                assert_eq!(out.q[i + 1], -2. * gain);
            }
        }
    }
}

#[test]
fn unsupported_has_spans() {
    for text in [
        "module m(p); electrical p; analog function real bad; input x; real x; begin bad=ddt(x); end endfunction endmodule",
        "module m(p); electrical p; analog V(<p>)<+0; endmodule",
        "module m(p); electrical p; analog I(p)<+ddx(V(p),V(p,0)); endmodule",
        "module m(p); electrical p; analog @(cross(V(p))) ; endmodule",
        "module m(p); electrical p; analog I(p)<+idt(V(p)); endmodule",
        "module m(p); electrical p; analog function real rec; input x; real x; begin rec=rec(x); end endfunction analog I(p)<+rec(1); endmodule",
    ] {
        let ast = parse_va(text, &NoVaFiles).unwrap();
        let e = compile_module(&ast, None).unwrap_err();
        assert!(
            e.iter()
                .all(|d| d.code == "E-VA-UNSUPPORTED" && d.span.is_some()),
            "{e:?}"
        );
    }
}
#[test]
fn semantic_errors_have_spans() {
    for text in [
        "module m(p); electrical p; analog I(p)<+missing; endmodule",
        "module m(p); electrical p; parameter real r=1; analog r=2; endmodule",
        "module m(p); electrical p; analog I(missing)<+1; endmodule",
        "module m(p); electrical p; analog I(p)<+exp(1,2); endmodule",
        "module m(p); electrical p; analog I(p)<+\"bad\"; endmodule",
    ] {
        let ast = parse_va(text, &NoVaFiles).unwrap();
        let e = compile_module(&ast, None).unwrap_err();
        assert!(
            e.iter().all(|d| d.code == "E-VA-SEMA" && d.span.is_some()),
            "{e:?}"
        );
    }
}

#[test]
fn parameter_defaults_aliases_and_integer_rounding() {
    let m = compile(
        r#"module defaults(p); electrical p;
 parameter real a=2 from [1:10], b=3*a; parameter integer count=2.5;
 localparam real c=b+1; aliasparam extra=b;
 analog I(p)<+a+b+c+count+10*$param_given(a)+100*$param_given(b);
 endmodule"#,
    );
    let b = m.bind(&[]).unwrap();
    assert_eq!(run(&m, &b, &[0.]).f, [18.]);
    let b = m.bind(&[("a".into(), 4.)]).unwrap();
    assert_eq!(run(&m, &b, &[0.]).f, [42.]);
    let b = m
        .bind_instance(&b, &[("extra".into(), 5.), ("count".into(), -2.5)])
        .unwrap();
    assert_eq!(run(&m, &b, &[0.]).f, [122.]);
    assert!(m.bind(&[("c".into(), 0.)]).is_err());
}
#[test]
fn distinct_parallel_named_branches_and_reversed_implicit_access() {
    let m = compile(
        r#"module reverse(p,n); electrical p,n; branch(p,n) forward; branch(n,p) backward;
 analog begin V(forward)<+2*I(forward); V(backward)<+-1; I(p,n)<+3; I(n,p)<+1; I(p,n)<+I(backward); end endmodule"#,
    );
    assert_eq!(m.branch_unknowns.len(), 2);
    let b = m.bind(&[]).unwrap();
    let bias = [4., 1., 0.5, 0.25];
    let o = run(&m, &b, &bias);
    assert_eq!(o.f, [2.5, -2.5, 2., -2.]);
    finite_difference(&m, &b, &bias);
}
#[test]
fn inout_nested_functions_and_local_scopes() {
    let m = compile(
        r#"module functions(p); electrical p; real y,r;
 analog function integer increment; inout a; integer a; begin a=a+1; increment=a; end endfunction
 analog function real outer; input a; real a; real b; begin b=a; outer=increment(b)+b; end endfunction
 analog begin : first real temp; temp=outer(3); y=3.9; r=increment(y); I(p)<+temp+r+y; end
 analog begin : second real temp; temp=2; I(p)<+temp; end
 endmodule"#,
    );
    let b = m.bind(&[]).unwrap();
    assert_eq!(run(&m, &b, &[0.]).f, [18.]);
}
#[test]
fn user_and_fet_limiters() {
    let m = compile(
        r#"module limiter(p); electrical p; real v;
 analog function real step; input fresh, old, delta; real fresh,old,delta; begin step=min(max(fresh,old-delta),old+delta); end endfunction
 analog begin v=$limit(V(p),"step",0.2); I(p)<+v+$limit(V(p),"fetlim",0.5); end endmodule"#,
    );
    let b = m.bind(&[]).unwrap();
    let mut state = [0., 0.];
    let mut out = EvalOutput::default();
    let input = EvalInput {
        x: &[3.],
        temperature: 300.,
        analysis: Analysis::Dc,
        initial_step: false,
        final_step: false,
        abstime: 0.,
        gmin: 0.,
        limit: true,
        simparams: &|_| None,
        mfactor: 1.,
    };
    m.eval(
        &b,
        &input,
        StateIo {
            accepted: &[0., 0.],
            current: &mut state,
        },
        &mut out,
    );
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(state, [0.2, 0.75]);
    assert_eq!(out.f, [6.]);
    assert_eq!(out.df, [2.]);
    assert!(out.noncon);
    // Non-finite caller history must not cause f64::clamp to panic.
    state = [f64::NAN; 2];
    m.eval(
        &b,
        &input,
        StateIo {
            accepted: &[0., 0.],
            current: &mut state,
        },
        &mut out,
    );
    assert_eq!(state, [0.2, 0.75]);
}
#[test]
fn environment_modes_and_limexp() {
    let m = compile(
        r#"module environment(p); electrical p; analog I(p)<+$temperature+$vt+$vt(400)+$abstime+$mfactor+$simparam("gmin",1)+$simparam("supplied",1)+analysis("ac")+analysis("static")+analysis("ic")+white_noise(1,"a")+flicker_noise(1,1,"b"); endmodule"#,
    );
    let b = m.bind(&[]).unwrap();
    let mut out = EvalOutput::default();
    let input = EvalInput {
        x: &[0.],
        temperature: 320.,
        analysis: Analysis::Ac,
        initial_step: true,
        final_step: false,
        abstime: 2.,
        gmin: 0.5,
        limit: false,
        simparams: &|n| if n == "supplied" { Some(3.) } else { None },
        mfactor: 4.,
    };
    m.eval(
        &b,
        &input,
        StateIo {
            accepted: &[],
            current: &mut [],
        },
        &mut out,
    );
    close(
        out.f[0],
        320. + 8.617333262145179e-5 * 720. + 2. + 4. + 0.5 + 3. + 2.,
    );
    let m = compile("module exponential(p); electrical p; analog I(p)<+limexp(V(p)); endmodule");
    let b = m.bind(&[]).unwrap();
    let o = run(&m, &b, &[82.]);
    assert_eq!(o.f, [3. * 80f64.exp()]);
    assert_eq!(o.df, [80f64.exp()]);
    finite_difference(&m, &b, &[79.]);
    finite_difference(&m, &b, &[82.]);
}
#[test]
fn scratch_and_invalid_shapes() {
    let m = compile("module scratch(p); electrical p; real v=2; analog I(p)<+v*V(p); endmodule");
    let b = m.bind(&[]).unwrap();
    let mut out = EvalOutput::default();
    let mut scratch = Scratch::default();
    let input = EvalInput {
        x: &[3.],
        temperature: 300.,
        analysis: Analysis::Dc,
        initial_step: false,
        final_step: false,
        abstime: 0.,
        gmin: 0.,
        limit: false,
        simparams: &|_| None,
        mfactor: 1.,
    };
    m.eval_with_scratch(
        &b,
        &input,
        StateIo {
            accepted: &[],
            current: &mut [],
        },
        &mut scratch,
        &mut out,
    );
    assert_eq!(out.f, [6.]);
    let bad = EvalInput { x: &[], ..input };
    m.eval(
        &b,
        &bad,
        StateIo {
            accepted: &[],
            current: &mut [],
        },
        &mut out,
    );
    assert!(out.error.is_some());
    let other =
        compile("module scratch(p); electrical p; real v=4; analog I(p)<+v*V(p); endmodule");
    let other_b = other.bind(&[]).unwrap();
    other.eval_with_scratch(
        &other_b,
        &input,
        StateIo {
            accepted: &[],
            current: &mut [],
        },
        &mut scratch,
        &mut out,
    );
    assert_eq!(out.f, [12.]);
    m.eval(
        &other_b,
        &input,
        StateIo {
            accepted: &[],
            current: &mut [],
        },
        &mut out,
    );
    assert!(out.error.is_some());
    fn assert_sync_send<T: Sync + Send>() {}
    assert_sync_send::<Bound>();
    assert_sync_send::<CompiledModule>();
}
#[test]
fn module_selection_and_scalar_type_errors() {
    let ast = parse_va("module a(); endmodule module b(); endmodule", &NoVaFiles).unwrap();
    assert_eq!(compile_module(&ast, None).unwrap().name, "b");
    assert_eq!(compile_module(&ast, Some("a")).unwrap().name, "a");
    assert!(compile_module(&ast, Some("absent")).is_err());
    for text in [
        "module m(p); electrical p; real a; analog a[0]=1; endmodule",
        "module m(p); electrical p; real a[0:1]; analog I(p)<+a; endmodule",
        "module m(p); electrical p; parameter string s=\"a\"; analog if(s) I(p)<+1; endmodule",
        "module m(p); electrical p; branch(b) b; analog V(b)<+0; endmodule",
    ] {
        let ast = parse_va(text, &NoVaFiles).unwrap();
        let err = compile_module(&ast, None).unwrap_err();
        assert!(
            err.iter()
                .all(|d| d.code == "E-VA-SEMA" && d.span.is_some()),
            "{err:?}"
        );
    }
}

#[test]
fn static_block_locals_and_output_index_capture() {
    let m = compile(
        r#"module stored(p); electrical p; analog begin : local real held; @(initial_step) held=7; I(p)<+held; end endmodule"#,
    );
    let b = m.bind(&[]).unwrap();
    let mut out = EvalOutput::default();
    let input = EvalInput {
        x: &[0.],
        temperature: 300.,
        analysis: Analysis::Dc,
        initial_step: true,
        final_step: false,
        abstime: 0.,
        gmin: 0.,
        limit: false,
        simparams: &|_| None,
        mfactor: 1.,
    };
    m.eval(
        &b,
        &input,
        StateIo {
            accepted: &[],
            current: &mut [],
        },
        &mut out,
    );
    assert_eq!(out.f, [7.]);
    assert_eq!(run(&m, &b, &[0.]).f, [7.]);
    let m = compile(
        r#"module copyback(p); electrical p; integer k; real array[0:1], result;
 analog function real change; input x; output y; real x,y; begin y=x; k=1; change=x; end endfunction
 analog begin k=0; array[0]=0; array[1]=0; result=change(5,array[k]); I(p)<+array[0]+10*array[1]; end endmodule"#,
    );
    let b = m.bind(&[]).unwrap();
    assert_eq!(run(&m, &b, &[0.]).f, [5.]);
}

#[test]
fn limited_diode_is_linearized_at_actual_voltage() {
    let m = compile(
        r#"module d(a,k); electrical a,k; real v;
        analog begin v=$limit(V(a,k),"pnjlim",0.02585,0.6);
        I(a,k)<+1e-14*(exp(v/0.02585)-1)+ddt(1e-9*1e-14*(exp(v/0.02585)-1));
        end endmodule"#,
    );
    let b = m.bind(&[]).unwrap();
    let mut current = [0.5];
    let mut out = EvalOutput::default();
    m.eval(
        &b,
        &EvalInput {
            x: &[2., 0.],
            temperature: 300.,
            analysis: Analysis::Dc,
            initial_step: false,
            final_step: false,
            abstime: 0.,
            gmin: 0.,
            limit: true,
            simparams: &|_| None,
            mfactor: 1.,
        },
        StateIo {
            accepted: &[0.5],
            current: &mut current,
        },
        &mut out,
    );
    let v = current[0];
    let exponential = (v / 0.02585).exp();
    let i = 1e-14 * (exponential - 1.);
    let g = 1e-14 * exponential / 0.02585;
    let expected = i + g * (2. - v);
    assert!(out.noncon && out.error.is_none());
    assert!((out.f[0] - expected).abs() <= 4. * f64::EPSILON * expected.abs());
    assert!((out.df[0] - g).abs() <= 4. * f64::EPSILON * g.abs());
    assert!((out.q[0] - 1e-9 * expected).abs() <= 4. * f64::EPSILON * (1e-9 * expected).abs());
}

#[test]
fn collapsed_diode_is_the_limit_of_the_resistive_layout() {
    let m = compile(include_str!("models/collapse.va"));
    let model = m.bind_model(&[], BindOptions::default()).unwrap();
    let short = m.bind_instance(&model, &[]).unwrap();
    let resistance = 1e-4;
    let series = m
        .bind_instance(&model, &[("rs".into(), resistance)])
        .unwrap();
    assert_eq!(short.layout().representatives(), &[0, 1]);
    assert_eq!(series.layout().representatives(), &[0, 1, 2]);
    let voltage = 0.45;
    let collapsed = run(&m, &short, &[voltage, 0.]);
    let current = collapsed.f[0];
    let finite = run(&m, &series, &[voltage + resistance * current, 0., voltage]);
    assert!((finite.f[0] - current).abs() < 1e-11);
    assert!(finite.f[2].abs() < 1e-11);
    assert_eq!(finite.f[1], collapsed.f[1]);
}

#[test]
fn dynamic_branch_is_preserved_at_zero_bias() {
    let m = compile(
        "module sw(p,n); electrical p,n; analog if(V(p)>1) I(p,n)<+2*V(p,n); else V(p,n)<+0; endmodule",
    );
    let b = m.bind(&[]).unwrap();
    assert_eq!(b.layout().unknown_len(), 3);
    assert_eq!(run(&m, &b, &[0., 0., 1.]).f, [1., -1., 0.]);
    assert_eq!(run(&m, &b, &[2., 0., 1.]).f, [1., -1., -3.]);
}

#[test]
fn connected_ports_and_flow_sensing() {
    let m = compile(
        "module sense(a,b); electrical a,b; branch (<a>) probe; analog begin I(a)<+2*V(a); I(b)<+I(probe)+I(<a>)+$port_connected(b); end endmodule",
    );
    let full = m.bind(&[]).unwrap();
    let open = m.bind_connected_instance(&full, &[], 1).unwrap();
    // External a,b, the inside of the sensing port, and positive current entering a.
    assert_eq!(run(&m, &full, &[2., 0., 2., 4.]).f, [4., 9., 0., 0.]);
    assert_eq!(run(&m, &open, &[2., 0., 2., 4.]).f, [4., 8., 0., 0.]);
    finite_difference(&m, &open, &[2., 0., 1.5, 3.8]);
}

#[test]
fn parameter_kinds_aliases_ambiguity_and_simulator_defaults() {
    let m = compile(
        r#"module settings(p); electrical p; (* desc="type=instance, *)", type="instance" *) parameter real Size=3;
        (* type="model" *) parameter real Heat=$simparam("tnom",0); aliasparam Width=Size;
        analog I(p)<+(Size+$given(Size))*$mfactor+Heat; endmodule"#,
    );
    let opts = BindOptions {
        tnom: 33.,
        gmin: 1e-11,
    };
    assert_eq!(
        m.parameter_info("width").unwrap().placement,
        Some(ParamKind::Instance)
    );
    assert_eq!(
        m.bind_model(&[("WIDTH".into(), 3.)], opts).unwrap_err()[0].code,
        "E-VA-PARAM"
    );
    let b = m.bind_model(&[], opts).unwrap();
    assert!(
        m.bind_instance(&b, &[("heat".into(), 2.)]).unwrap_err()[0]
            .message
            .contains("model parameter")
    );
    let i = m
        .bind_instance(&b, &[("wIdTh".into(), 5.), ("M".into(), 2.)])
        .unwrap();
    assert_eq!(run(&m, &i, &[0.]).f, [45.]);
    let d = m.bind_instance(&b, &[("sise".into(), 1.)]).unwrap_err();
    assert!(d[0].message.contains("did you mean Size"));
    let m = compile("module ambiguous(); parameter real val=0, VAL=1; endmodule");
    assert!(
        m.bind(&[("Val".into(), 2.)]).unwrap_err()[0]
            .message
            .contains("ambiguous")
    );
}

#[test]
fn final_events_noise_tables_and_unused_random_calls() {
    let m = compile(
        r#"module events(p); electrical p; integer seed; real a;
        analog begin @(initial_model or initial_instance) a=1; @(final_step) a=2;
        I(p)<+a+noise_table({1,2,10,0.3},"spectrum");
        if (V(p)>0) I(p)<+$rdist_normal(seed,0,1);
        $discontinuity(1); $bound_step(1n); end endmodule"#,
    );
    let b = m.bind(&[]).unwrap();
    let input = EvalInput {
        x: &[0.],
        temperature: 300.,
        analysis: Analysis::Dc,
        initial_step: true,
        final_step: false,
        abstime: 0.,
        gmin: 0.,
        limit: false,
        simparams: &|_| None,
        mfactor: 1.,
    };
    let mut o = EvalOutput::default();
    m.eval(
        &b,
        &input,
        StateIo {
            accepted: &[],
            current: &mut [],
        },
        &mut o,
    );
    assert_eq!(o.f, [1.]);
    m.eval(
        &b,
        &EvalInput {
            initial_step: false,
            final_step: true,
            ..input
        },
        StateIo {
            accepted: &[],
            current: &mut [],
        },
        &mut o,
    );
    assert_eq!(o.f, [2.]);
    m.eval(
        &b,
        &EvalInput { x: &[1.], ..input },
        StateIo {
            accepted: &[],
            current: &mut [],
        },
        &mut o,
    );
    let d = o.diagnostic.unwrap();
    assert_eq!(d.code, "E-VA-UNSUPPORTED");
    assert!(d.span.is_some());
}

#[test]
fn switching_bindings_restarts_instance_initialization() {
    let m = compile(
        r#"module startup(p); electrical p; real t; analog begin @(initial_instance) t=$simparam("tnom"); I(p)<+t; end endmodule"#,
    );
    let mut scratch = Scratch::default();
    let mut out = EvalOutput::default();
    for tnom in [20., 55.] {
        let b = m
            .bind_model(
                &[],
                BindOptions {
                    tnom,
                    ..BindOptions::default()
                },
            )
            .unwrap();
        m.eval_with_scratch(
            &b,
            &EvalInput {
                x: &[0.],
                temperature: 310.,
                analysis: Analysis::Dc,
                initial_step: false,
                final_step: false,
                abstime: 0.,
                gmin: 0.,
                limit: false,
                simparams: &|_| None,
                mfactor: 1.,
            },
            StateIo {
                accepted: &[],
                current: &mut [],
            },
            &mut scratch,
            &mut out,
        );
        assert_eq!(out.f, [tnom]);
    }
}

#[test]
fn collecting_loop_temporaries_preserves_caller_gradients() {
    let m = compile(
        r#"module accum(p,a,b,c,d,e,f,g);
      electrical p,a,b,c,d,e,f,g;
      analog function real sum; input v; real v,t; integer j;
        begin t=0; for(j=0;j<5000;j=j+1) t=t+v+v+v+v+v+v+v+v; sum=t; end
      endfunction
      analog I(p)<+2.0*V(p)+sum(V(p));
    endmodule"#,
    );
    let b = m.bind(&[]).unwrap();
    for bias in [0.5, -1.] {
        let out = run(&m, &b, &[bias; 8]);
        assert_eq!(out.f[0], 40002. * bias);
        assert_eq!(out.df[0], 40002.);
    }
}

/// Measure dispatch, initialization of stamp slots, native return and validation
/// with a seven-unknown circuit layout and an empty analog block.
#[cfg(feature = "jit")]
#[test]
#[ignore = "release microbenchmark"]
fn empty_kernel_overhead() {
    let m = compile(
        "module empty(a,b,c,d,e,f,g); electrical a,b,c,d,e,f,g; analog begin end endmodule",
    );
    let mut b = m.bind(&[]).unwrap();
    b.specialize_stamp(&(0..7).map(Some).collect::<Vec<_>>(), false);
    let input = EvalInput {
        x: &[0.; 7],
        temperature: 300.,
        analysis: Analysis::Tran,
        initial_step: false,
        final_step: false,
        abstime: 1.,
        gmin: 1e-12,
        limit: false,
        simparams: &|_| None,
        mfactor: 1.,
    };
    let mut workspace = Scratch::default();
    let mut out = EvalOutput::default();
    let mut times = Vec::new();
    for _ in 0..8 {
        let start = std::time::Instant::now();
        for _ in 0..100_000 {
            m.eval_with_scratch(
                &b,
                std::hint::black_box(&input),
                StateIo {
                    accepted: &[],
                    current: &mut [],
                },
                &mut workspace,
                std::hint::black_box(&mut out),
            );
        }
        times.push(start.elapsed().as_nanos() as f64 / 100_000.);
    }
    assert!(workspace.used_jit() && out.error.is_none());
    times[1..].sort_by(f64::total_cmp);
    eprintln!("empty native evaluation: {:.1} ns/call", times[4]);
}

#[cfg(feature = "jit")]
#[test]
fn packed_instances_match_individual_stamps_on_divergent_paths() {
    let m = compile(
        r#"module branches(p,q,z); electrical p,q,z;
        real u,w;
        analog begin
          if (V(p)>0) begin
            u=sqrt(V(p))+$simparam("gmin",1);
            w=u+V(q);
            if (V(q)>0) begin
              w=u*exp(V(q));
              if (V(q)>0.5) w=w*V(q)+cos(V(p))+1;
              w=w*w+u;
            end
            I(p,q)<+w;
          end else begin
            u=sqrt(-V(p));
            if (V(q)>1) w=u*V(q); else w=u-2;
            I(q)<+w;
          end
          I(p)<+ddt(w*V(p));
        end endmodule"#,
    );
    let mut b = m.bind(&[]).unwrap();
    b.specialize_stamp(&[Some(0), Some(1), None], false);
    let pair = m.prepare_circuit_pair(&b, 300.).unwrap();
    let projected = m.prepare_projected_pair(&b, 300., &[1]).unwrap();
    let mut packed = EvalOutput::default();
    for (p, q) in [
        ([0.4, -0.3], [-0.1, 2.]),
        ([-0.5, -0.7], [0.3, 1.2]),
        ([0.3, 0.7], [0.4, 0.8]),
    ] {
        assert!(pair.evaluate(&[p[0], p[1], q[0], q[1], 0., 0.], 1e-12, &mut packed));
        let mut reduced = EvalOutput::default();
        assert!(projected.evaluate(&[p[0], p[1], q[0], q[1], 0., 0.], 1e-12, &mut reduced));
        for (full, constrained) in [(&packed.f, &reduced.f), (&packed.q, &reduced.q)] {
            assert!(
                full.iter()
                    .zip(constrained)
                    .all(|(x, y)| x.to_bits() == y.to_bits())
            );
        }
        for (full, constrained) in [(&packed.df, &reduced.df), (&packed.dq, &reduced.dq)] {
            for (i, (x, y)) in full.iter().zip(constrained).enumerate() {
                let expected = if i / 2 % 3 == 1 { 0. } else { *x };
                assert_eq!(expected.to_bits(), y.to_bits());
            }
        }
        for lane in 0..2 {
            let mut out = EvalOutput::default();
            let input = EvalInput {
                x: &[p[lane], q[lane], 0.],
                temperature: 300.,
                analysis: Analysis::Tran,
                initial_step: false,
                final_step: false,
                abstime: 1.,
                gmin: 1e-12,
                limit: false,
                simparams: &|_| None,
                mfactor: 1.,
            };
            m.eval_with_scratch(
                &b,
                &input,
                StateIo {
                    accepted: &[],
                    current: &mut [],
                },
                &mut Scratch::default(),
                &mut out,
            );
            for (v, w) in [
                (&packed.f, &out.f),
                (&packed.q, &out.q),
                (&packed.df, &out.df),
                (&packed.dq, &out.dq),
            ] {
                assert!(
                    v.iter()
                        .skip(lane)
                        .step_by(2)
                        .zip(w)
                        .all(|(a, b)| a.to_bits() == b.to_bits())
                );
            }
        }
    }
    assert!(!pair.evaluate(&[1., 2., 0., 0., 1., 0.], 1e-12, &mut packed));
    assert!(!pair.evaluate(&[f64::NAN, 2., 0., 0., 0., 0.], 1e-12, &mut packed));
}

#[cfg(feature = "jit")]
#[test]
fn packed_prediction_replays_only_for_an_active_mismatch() {
    // The inner threshold lies beyond the predictor's training biases.
    let module = compile(
        "module m(p,q); electrical p,q; analog begin if(V(q)>0) begin if(V(p)>200) I(p)<+2*V(p); else I(p)<+V(p); end end endmodule",
    );
    let mut bound = module.bind(&[]).unwrap();
    bound.specialize_stamp(&[Some(0), Some(1)], false);
    let pair = module.prepare_circuit_pair(&bound, 300.).unwrap();
    let mut stamp = EvalOutput::default();
    assert!(pair.evaluate(&[250., 1., -1., 1.], 1e-12, &mut stamp));
    assert_eq!(&stamp.f[..2], &[0., 1.]);
    assert!(!pair.evaluate(&[250., 1., 1., 1.], 1e-12, &mut stamp));
}
