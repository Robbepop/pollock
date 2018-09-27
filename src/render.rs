use state::{DrawParams, ExtendedState, Fill, Scale, StateWithModifications, Stroke};
use std::iter::{self, ExactSizeIterator};
use {v2, Color, ColorFormat, V2};

use gfx;

macro_rules! fixed_size_iter {
    ($one:expr) => {
        ::std::iter::once($one)
    };
    ($first:expr$(, $next:expr)+) => {
        fixed_size_iter!($first).chain(fixed_size_iter!($($next),*))
    }
}

pub mod shaders {
    pub const VERTEX_SHADER: &str = r##"
#version 150 core

in vec4 a_Pos;
in vec4 a_Color;
in vec2 a_Normal;

uniform Transform {
    mat4 u_Transform;
};

out vec4 v_Color;

void main() {
    v_Color = a_Color;
    vec4 new_Pos = vec4(a_Pos.xy + a_Normal, a_Pos.zw);
    gl_Position = new_Pos * u_Transform;
}
"##;

    pub const FRAGMENT_SHADER: &str = r##"
#version 150 core

in vec4 v_Color;
out vec4 Target0;

void main() {
    Target0 = v_Color;
}
"##;
}

// Put this code above your main function
gfx_defines!{
    vertex Vertex {
        pos: [f32; 4] = "a_Pos",
        color: [f32; 4] = "a_Color",
        normal: [f32; 2] = "a_Normal",
    }

    constant Transform {
        transform: [[f32; 4]; 4] = "u_Transform",
    }

    pipeline pipe {
        vbuf: gfx::VertexBuffer<Vertex> = (),
        transform: gfx::ConstantBuffer<Transform> = "Transform",
        out: gfx::BlendTarget<ColorFormat> = (
            "Target0",
            gfx::state::ColorMask::all(),
            gfx::preset::blend::ALPHA
        ),
    }
}

#[derive(Default)]
pub struct DrawState {
    pub(crate) vertices: Vec<Vertex>,
    pub(crate) indices: Vec<u32>,
}

impl DrawState {
    pub(crate) fn new(vertices: Vec<Vertex>, indices: Vec<u32>) -> Self {
        DrawState { vertices, indices }
    }

    /// Draw any convex polygon without lines
    #[inline]
    fn convex_poly<I>(&mut self, stroke: Stroke, fill: Fill, verts: I)
    where
        I: Iterator<Item = V2WithNorm> + ExactSizeIterator,
    {
        use itertools::Itertools;

        if fill.is_none() {
            return;
        }

        let half_t = stroke.thickness / 2.;
        let start = self.vertices.len() as u32;
        let fill_color = [
            fill.color.color.red as f32 / 255.,
            fill.color.color.green as f32 / 255.,
            fill.color.color.blue as f32 / 255.,
            fill.color.alpha as f32 / 255.,
        ];
        let len = verts.len();
        self.vertices.extend(verts.map(|v| {
            let norm = v.norm * half_t;
            Vertex {
                pos: [v.pos.x as f32, v.pos.y as f32, 0., 1.0],
                color: fill_color,
                normal: [norm.x as f32, norm.y as f32],
            }
        }));
        self.indices.extend(
            (1..len)
                .tuple_windows()
                .flat_map(|(b, c)| fixed_size_iter![start, start + b as u32, start + c as u32]),
        );
    }

    #[inline]
    fn lines<I>(&mut self, stroke: Stroke, verts: I, connect_back: bool)
    where
        I: Iterator<Item = V2WithNorm>,
    {
        use itertools::Itertools;

        if stroke.is_none() {
            return;
        }

        let half_t = stroke.thickness / 2.;
        let start = self.vertices.len() as u32;
        let stroke_color = [
            stroke.color.color.red as f32 / 255.,
            stroke.color.color.green as f32 / 255.,
            stroke.color.color.blue as f32 / 255.,
            stroke.color.alpha as f32 / 255.,
        ];

        self.vertices.extend(verts.flat_map(|v| {
            let norm = v.norm * half_t;
            iter::once(Vertex {
                pos: [v.pos.x as f32, v.pos.y as f32, 0., 1.0],
                color: stroke_color,
                normal: [norm.x as f32, norm.y as f32],
            }).chain(iter::once(Vertex {
                pos: [v.pos.x as f32, v.pos.y as f32, 0., 1.0],
                color: stroke_color,
                normal: [-norm.x as f32, -norm.y as f32],
            }))
        }));

        // We only do this so we don't have to add an ExactSizeIterator bound to `lines`
        let count = (self.vertices.len() as u32 - start) / 2;

        self.indices.extend(
            (0..count)
                .chain(if connect_back { Some(0) } else { None })
                .tuple_windows()
                .flat_map(|(cur, next)| {
                    let first = cur * 2;
                    let next = next * 2;
                    fixed_size_iter![
                        first,
                        first + 1,
                        next,
                        first + 1,
                        next,
                        next + 1 // TODO: HACK - we add an extra face here
                                 //       because I can't get the normal vertices
                                 //       to point a consistent direction
                                 //       (relative to the line).
                                 //
                                 //       When we release, ask for help with
                                 //       this.
                                 // first,
                                 // first + 1,
                                 // next + 1
                    ].map(|i| i + start)
                }),
        );
    }
}

#[derive(Copy, Clone)]
struct V2WithNorm {
    pos: V2,
    norm: V2,
}

impl V2WithNorm {
    fn new(pos: V2, norm: V2) -> Self {
        V2WithNorm { pos, norm }
    }
}

fn miter(last: V2, cur: V2, next: V2) -> V2 {
    let (last_edge, next_edge) = (last - cur, next - cur);
    let (last_edge, next_edge) = (last_edge.normalize(), next_edge.normalize());
    let tangent = (last_edge + next_edge).normalize();

    let miter = tangent;
    let tmp = v2(-last_edge.y, last_edge.x);
    let multiplier = ((-last_edge.dot(&next_edge)).acos() / 2.).cos();

    miter * miter.dot(&tmp).signum() as f64 / multiplier
}

impl<'a, S> ExtendedState<'a, S, DrawState>
where
    S: DrawParams,
{
    /// Draw any convex polygon with lines. This is used by `rect`, `ellipse` etc.
    /// We can probably extract out just the fill so that we can use this to draw
    /// components of a concave polygon too.
    fn convex_poly<I>(&self, verts: I)
    where
        I: Iterator<Item = V2WithNorm> + ExactSizeIterator + Clone,
    {
        let mut inner = self.inner.borrow_mut();
        inner.convex_poly(self.stroke(), self.fill(), verts.clone());
        inner.lines(self.stroke(), verts, true);
    }

    pub fn set(&self, _pos: V2, _color: Color) {
        // TODO: Draw a single small square here - very inefficient but simple to implement and
        //       software rendering like this was never going to be efficient.
        unimplemented!()
    }

    pub fn circle<R: Into<f64>>(&self, pos: V2, rad: R) {
        let rad = rad.into();
        self.ellipse(pos, rad, rad)
    }

    pub fn ellipse<RX: Into<f64>, RY: Into<f64>>(&self, pos: V2, rad_x: RX, rad_y: RY) {
        use std::f32::consts::PI;

        // TODO: Make this configurable somehow?
        let (r_x, r_y) = (rad_x.into() as f32, rad_y.into() as f32);
        let circumference = 2. * PI * ((r_x * r_x + r_y * r_y) / 2.).sqrt();
        let resolution = ((circumference / 15.) as u32).max(12);
        let angle_step = 2. * PI / resolution as f32;

        self.convex_poly((0..resolution).map(|i| {
            let angle = i as f32 * angle_step;
            let (s, c) = (angle.sin(), angle.cos());

            V2WithNorm::new(pos + v2(s * r_x, c * r_y), v2(-s, -c))
        }));
    }

    pub fn rect<W: Into<f64>, H: Into<f64>>(&self, pos: V2, width: W, height: H) {
        use std::f32::consts::SQRT_2;

        let (x, y, w, h) = (
            pos.x as f32,
            pos.y as f32,
            width.into() as f32,
            height.into() as f32,
        );

        let top_left = V2WithNorm::new(v2(x, y), v2(SQRT_2, SQRT_2));
        let top_right = V2WithNorm::new(v2(x + w, y), v2(-SQRT_2, SQRT_2));
        let bottom_left = V2WithNorm::new(v2(x, y + h), v2(SQRT_2, -SQRT_2));
        let bottom_right = V2WithNorm::new(v2(x + w, y + h), v2(-SQRT_2, -SQRT_2));

        self.convex_poly(
            [top_left, top_right, bottom_right, bottom_left]
                .iter()
                .cloned(),
        );
    }

    #[inline]
    pub fn line(&self, a: V2, b: V2) {
        let diff = b - a;
        let norm = v2(-diff.x, diff.y);
        self.inner.borrow_mut().lines(
            self.stroke(),
            fixed_size_iter![V2WithNorm { pos: a, norm }, V2WithNorm { pos: b, norm }],
            false,
        );
    }

    #[inline]
    pub fn lines<I: IntoIterator<Item = V2>>(&self, verts: I) {
        use itertools::Itertools;
        // TODO: Use simple joins (avoid calculating properly) when stroke thickness is
        //       small.
        self.inner.borrow_mut().lines(
            self.stroke(),
            iter::once(None)
                .chain(verts.into_iter().map(Some))
                .chain(iter::once(None))
                .tuple_windows()
                .map(|(last, cur, next)| {
                    // TODO: Use unchecked unwrap?
                    let cur = cur.unwrap();
                    let norm = match (last, next) {
                        (None, Some(next)) => {
                            let edge = next - cur;
                            v2(edge.y, -edge.x).normalize()
                        }
                        (Some(last), None) => {
                            let edge = cur - last;

                            v2(edge.y, -edge.x).normalize()
                        }
                        (Some(last), Some(next)) => miter(last, cur, next),
                        _ => unreachable!(),
                    };

                    V2WithNorm { pos: cur, norm }
                }),
            false,
        )
    }

    #[inline]
    pub fn polygon<I>(&self, verts: I)
    where
        I: IntoIterator<Item = V2>,
    {
        use itertools::Itertools;
        if !self.fill().is_none() {
            unimplemented!(
                "I haven't unimplemented decomposing concave polygons into convex ones yet"
            );
        }

        let mut verts = verts.into_iter();
        let first = verts.next();
        let second = verts.next();

        // TODO: Use simple joins (avoid calculating properly) when stroke thickness is
        //       small.
        self.inner.borrow_mut().lines(
            self.stroke(),
            first
                .into_iter()
                .chain(second)
                .chain(verts)
                .chain(first)
                .chain(second)
                .tuple_windows()
                .map(|(last, cur, next)| V2WithNorm {
                    pos: cur,
                    norm: miter(last, cur, next),
                }),
            true,
        );
    }

    pub fn push(&self) -> ExtendedState<StateWithModifications<S>, DrawState> {
        ExtendedState {
            state: StateWithModifications::new(&self.state),
            inner: self.inner,
        }
    }

    // We don't use `Into<f64>` here because that encourages people accidentally supplying degrees
    // Obviously they'll realise pretty quick but there's no reason to ever supply radians as a
    // type other than f64.
    pub fn with_rotate(&self, radians: f64) -> ExtendedState<StateWithModifications<S>, DrawState> {
        let mut out = self.push();
        out.transform *= ::Transform::new_rotation(radians);
        out
    }

    pub fn with_translate(
        &self,
        translate: V2,
    ) -> ExtendedState<StateWithModifications<S>, DrawState> {
        let mut out = self.push();
        out.transform *= ::Transform::new_translation(&translate);
        out
    }

    pub fn with_scale<Scl: Scale>(
        &self,
        scale: Scl,
    ) -> ExtendedState<StateWithModifications<S>, DrawState> {
        let mut out = self.push();
        out.transform *= ::Transform::new_nonuniform_scaling(&scale.into_scale());
        out
    }

    pub fn with_stroke(
        &self,
        stroke: Stroke,
    ) -> ExtendedState<StateWithModifications<S>, DrawState> {
        let mut out = self.push();
        out.stroke = stroke;
        out
    }

    pub fn with_fill(&self, fill: Fill) -> ExtendedState<StateWithModifications<S>, DrawState> {
        let mut out = self.push();
        out.fill = fill;
        out
    }
}