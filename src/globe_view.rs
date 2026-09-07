//! `GlobeView` — the optional 3D globe view, following Apple's
//! `MKMapView` globe elevation mode.
//!
//! The earth is rendered as a textured sphere with OpenGL through GTK's
//! `GLArea`. The earth texture is stitched live from Web Mercator tiles of
//! the configured provider (dark style gets the dark CARTO basemap), so the
//! globe needs no bundled imagery. When tiles cannot be fetched the view
//! falls back to a procedural ocean-and-graticule shading.
//!
//! Interaction: drag rotates the globe, scroll zooms, and an optional
//! auto-rotate mode slowly spins the earth. Markers are drawn as accent
//! colored dots on the front hemisphere.
//!
//! ```rust,no_run
//! use mapskit::prelude::*;
//!
//! let globe = GlobeView::new(&MapsConfiguration::new().style(MapStyle::Dark));
//! globe.set_center(Coordinate::new(48.137, 11.575));
//! globe.add_marker(GlobeMarker::new(Coordinate::new(48.137, 11.575), "Munich"));
//! globe.set_auto_rotate(true);
//! ```

use crate::config::MapsConfiguration;
use crate::providers::ProviderChain;
use crate::types::Coordinate;

use gtk::prelude::*;
use gtk::{GLArea, EventControllerScroll, GestureDrag};
use glow::HasContext;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

type Gl = glow::Context;
type GlProgram = <Gl as HasContext>::Program;
type GlUniformLocation = <Gl as HasContext>::UniformLocation;
type GlVertexArray = <Gl as HasContext>::VertexArray;
type GlBuffer = <Gl as HasContext>::Buffer;

/// A dot marker on the globe.
#[derive(Debug, Clone, PartialEq)]
pub struct GlobeMarker {
    pub coordinate: Coordinate,
    pub label: String,
}

impl GlobeMarker {
    pub fn new(coordinate: Coordinate, label: impl Into<String>) -> Self {
        Self {
            coordinate,
            label: label.into(),
        }
    }
}

const MAX_MARKERS: usize = 64;
const SPHERE_LAT_BANDS: usize = 64;
const SPHERE_LON_SEGS: usize = 128;

/// Closest camera distance (surface nearly fills the view).
const MIN_DISTANCE: f64 = 1.15;
/// Farthest camera distance (whole globe visible).
const MAX_DISTANCE: f64 = 6.0;

const GLOBE_VERTEX_SHADER: &str = r"#version 330 core
in vec3 a_pos;
uniform mat4 u_mvp;
uniform mat4 u_model;
out vec3 v_normal;
void main() {
    v_normal = a_pos;
    gl_Position = u_mvp * vec4(a_pos, 1.0);
}
";

const GLOBE_FRAGMENT_SHADER: &str = r"#version 330 core
in vec3 v_normal;
uniform sampler2D u_earth;
uniform sampler2D u_detail;
uniform bool u_has_texture;
uniform bool u_has_detail;
// Region of the detail texture as (lon_min, lon_max, merc_y_top, merc_y_bottom).
uniform vec4 u_detail_region;
uniform vec3 u_light_dir;
uniform mat4 u_model;
out vec4 frag;

const float PI = 3.14159265358979;

void main() {
    vec3 n = normalize(v_normal);
    float lat = asin(clamp(n.y, -1.0, 1.0));
    float lon = atan(n.x, n.z);

    float lat_c = clamp(lat, radians(-85.0511), radians(85.0511));
    float my = log(tan(PI / 4.0 + lat_c / 2.0));

    vec3 base;
    if (u_has_texture) {
        vec2 uv = vec2(lon / (2.0 * PI) + 0.5, 0.5 - my / (2.0 * PI));
        base = texture(u_earth, uv).rgb;
    } else {
        float lat_deg = degrees(lat);
        float lon_deg = degrees(lon);
        float t = clamp((lat_deg + 90.0) / 180.0, 0.0, 1.0);
        base = mix(vec3(0.02, 0.09, 0.20), vec3(0.04, 0.21, 0.40), t);
        if (mod(lat_deg + 90.0, 15.0) < 1.2 || mod(lon_deg + 180.0, 15.0) < 1.2) {
            base += 0.05;
        }
        float polar = smoothstep(76.0, 84.0, abs(lat_deg));
        base = mix(base, vec3(0.88, 0.92, 0.97), polar);
    }

    // Sharper regional imagery wins where it covers the fragment.
    if (u_has_detail) {
        float u = (lon - u_detail_region.x) / (u_detail_region.y - u_detail_region.x);
        float v = (u_detail_region.z - my) / (u_detail_region.z - u_detail_region.w);
        if (u >= 0.0 && u <= 1.0 && v >= 0.0 && v <= 1.0) {
            base = texture(u_detail, vec2(u, v)).rgb;
        }
    }

    vec3 wn = normalize(mat3(u_model) * n);
    float diff = max(dot(wn, normalize(u_light_dir)), 0.0);
    float facing = max(wn.z, 0.0);
    float rim = pow(1.0 - facing, 3.0);

    vec3 color = base * (0.36 + diff * 0.80);
    color += vec3(0.30, 0.55, 1.00) * rim * 0.45;
    frag = vec4(color, 1.0);
}
";

const MARKER_VERTEX_SHADER: &str = r"#version 330 core
in vec3 a_pos;
uniform mat4 u_mvp;
uniform mat4 u_model;
uniform float u_size;
out float v_visible;
void main() {
    vec3 wn = normalize(mat3(u_model) * a_pos);
    v_visible = step(0.10, wn.z);
    gl_Position = u_mvp * vec4(a_pos * 1.01, 1.0);
    gl_PointSize = u_size;
}
";

const MARKER_FRAGMENT_SHADER: &str = r"#version 330 core
in float v_visible;
uniform vec3 u_color;
out vec4 frag;
void main() {
    frag = vec4(u_color, v_visible);
}
";

// ═══════════════════════════════════════════════════════════════
// Minimal column-major mat4 math
// ═══════════════════════════════════════════════════════════════

type Mat4 = [f32; 16];

fn mat_identity() -> Mat4 {
    [
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    ]
}

fn mat_multiply(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.0_f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a[k * 4 + row] * b[col * 4 + k];
            }
            out[col * 4 + row] = sum;
        }
    }
    out
}

fn mat_perspective(fov_y_radians: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fov_y_radians / 2.0).tan();
    let nf = 1.0 / (near - far);
    [
        f / aspect,
        0.0,
        0.0,
        0.0, //
        0.0,
        f,
        0.0,
        0.0, //
        0.0,
        0.0,
        (far + near) * nf,
        -1.0, //
        0.0,
        0.0,
        2.0 * far * near * nf,
        0.0,
    ]
}

fn mat_rotation_x(radians: f32) -> Mat4 {
    let (s, c) = radians.sin_cos();
    [
        1.0, 0.0, 0.0, 0.0, //
        0.0, c, s, 0.0, //
        0.0, -s, c, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    ]
}

fn mat_rotation_y(radians: f32) -> Mat4 {
    let (s, c) = radians.sin_cos();
    [
        c, 0.0, -s, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        s, 0.0, c, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    ]
}

fn mat_translation_z(z: f32) -> Mat4 {
    let mut m = mat_identity();
    m[14] = z;
    m
}

/// Unit sphere position for lat/lon in degrees.
fn sphere_point(latitude: f64, longitude: f64) -> [f32; 3] {
    let phi = latitude.to_radians();
    let lambda = longitude.to_radians();
    [
        (phi.cos() * lambda.sin()) as f32,
        phi.sin() as f32,
        (phi.cos() * lambda.cos()) as f32,
    ]
}

fn build_sphere_mesh() -> (Vec<[f32; 3]>, Vec<u32>) {
    let mut positions = Vec::with_capacity((SPHERE_LAT_BANDS + 1) * (SPHERE_LON_SEGS + 1));
    for band in 0..=SPHERE_LAT_BANDS {
        let lat = 90.0 - (band as f64 / SPHERE_LAT_BANDS as f64) * 180.0;
        for seg in 0..=SPHERE_LON_SEGS {
            let lon = (seg as f64 / SPHERE_LON_SEGS as f64) * 360.0 - 180.0;
            positions.push(sphere_point(lat, lon));
        }
    }
    let stride = (SPHERE_LON_SEGS + 1) as u32;
    let mut indices = Vec::with_capacity(SPHERE_LAT_BANDS * SPHERE_LON_SEGS * 6);
    for band in 0..SPHERE_LAT_BANDS as u32 {
        for seg in 0..SPHERE_LON_SEGS as u32 {
            let top_left = band * stride + seg;
            let top_right = top_left + 1;
            let bottom_left = top_left + stride;
            let bottom_right = bottom_left + 1;
            indices.extend_from_slice(&[
                top_left,
                bottom_left,
                top_right,
                top_right,
                bottom_left,
                bottom_right,
            ]);
        }
    }
    (positions, indices)
}

/// Reinterprets a float/int slice as raw bytes for GL uploads.
fn as_bytes<T>(slice: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, std::mem::size_of_val(slice)) }
}

// ═══════════════════════════════════════════════════════════════
// GL resources
// ═══════════════════════════════════════════════════════════════

struct GlResources {
    globe_program: GlProgram,
    marker_program: GlProgram,
    sphere_vao: GlVertexArray,
    index_count: i32,
    marker_vao: GlVertexArray,
    marker_vbo: GlBuffer,
    earth_texture: Option<glow::Texture>,
    /// Sharper regional imagery with its bounds
    /// `(lon_min, lon_max, merc_y_top, merc_y_bottom)` in radians.
    detail_texture: Option<(glow::Texture, [f32; 4])>,
    uniforms: GlobeUniforms,
}

#[derive(Default)]
struct GlobeUniforms {
    globe_mvp: Option<GlUniformLocation>,
    globe_model: Option<GlUniformLocation>,
    globe_light: Option<GlUniformLocation>,
    has_texture: Option<GlUniformLocation>,
    earth_sampler: Option<GlUniformLocation>,
    has_detail: Option<GlUniformLocation>,
    detail_sampler: Option<GlUniformLocation>,
    detail_region: Option<GlUniformLocation>,
    marker_mvp: Option<GlUniformLocation>,
    marker_model: Option<GlUniformLocation>,
    marker_size: Option<GlUniformLocation>,
    marker_color: Option<GlUniformLocation>,
}

fn compile_program(gl: &Gl, vertex_src: &str, fragment_src: &str) -> Result<GlProgram, String> {
    unsafe {
        let program = gl.create_program().map_err(|e| e.to_string())?;

        let vert = gl.create_shader(glow::VERTEX_SHADER).map_err(|e| e.to_string())?;
        gl.shader_source(vert, vertex_src);
        gl.compile_shader(vert);
        if !gl.get_shader_compile_status(vert) {
            let log = gl.get_shader_info_log(vert);
            return Err(format!("vertex shader: {log}"));
        }

        let frag = gl
            .create_shader(glow::FRAGMENT_SHADER)
            .map_err(|e| e.to_string())?;
        gl.shader_source(frag, fragment_src);
        gl.compile_shader(frag);
        if !gl.get_shader_compile_status(frag) {
            let log = gl.get_shader_info_log(frag);
            return Err(format!("fragment shader: {log}"));
        }

        gl.attach_shader(program, vert);
        gl.attach_shader(program, frag);
        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            let log = gl.get_program_info_log(program);
            return Err(format!("link: {log}"));
        }
        gl.delete_shader(vert);
        gl.delete_shader(frag);
        Ok(program)
    }
}

impl GlResources {
    fn new(gl: &Gl) -> Result<Self, String> {
        unsafe {
            let globe_program = compile_program(gl, GLOBE_VERTEX_SHADER, GLOBE_FRAGMENT_SHADER)?;
            let marker_program =
                compile_program(gl, MARKER_VERTEX_SHADER, MARKER_FRAGMENT_SHADER)?;

            let (positions, indices) = build_sphere_mesh();

            let sphere_vao = gl.create_vertex_array().map_err(|e| e.to_string())?;
            gl.bind_vertex_array(Some(sphere_vao));

            let position_vbo = gl.create_buffer().map_err(|e| e.to_string())?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(position_vbo));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                as_bytes(&positions),
                glow::STATIC_DRAW,
            );
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 0, 0);

            let index_ibo = gl.create_buffer().map_err(|e| e.to_string())?;
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(index_ibo));
            gl.buffer_data_u8_slice(
                glow::ELEMENT_ARRAY_BUFFER,
                as_bytes(&indices),
                glow::STATIC_DRAW,
            );

            let marker_vao = gl.create_vertex_array().map_err(|e| e.to_string())?;
            gl.bind_vertex_array(Some(marker_vao));
            let marker_vbo = gl.create_buffer().map_err(|e| e.to_string())?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(marker_vbo));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                &[0u8; MAX_MARKERS * 12],
                glow::DYNAMIC_DRAW,
            );
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 0, 0);

            gl.bind_vertex_array(None);

            let uni =
                |program: &GlProgram, name: &str| gl.get_uniform_location(*program, name);
            let uniforms = GlobeUniforms {
                globe_mvp: uni(&globe_program, "u_mvp"),
                globe_model: uni(&globe_program, "u_model"),
                globe_light: uni(&globe_program, "u_light_dir"),
                has_texture: uni(&globe_program, "u_has_texture"),
                earth_sampler: uni(&globe_program, "u_earth"),
                has_detail: uni(&globe_program, "u_has_detail"),
                detail_sampler: uni(&globe_program, "u_detail"),
                detail_region: uni(&globe_program, "u_detail_region"),
                marker_mvp: uni(&marker_program, "u_mvp"),
                marker_model: uni(&marker_program, "u_model"),
                marker_size: uni(&marker_program, "u_size"),
                marker_color: uni(&marker_program, "u_color"),
            };

            Ok(Self {
                globe_program,
                marker_program,
                sphere_vao,
                index_count: indices.len() as i32,
                marker_vao,
                marker_vbo,
                earth_texture: None,
                detail_texture: None,
                uniforms,
            })
        }
    }

    /// Uploads RGBA8 bytes as a mip-mapped texture and returns its handle.
    fn upload_rgba_texture(
        gl: &Gl,
        rgba: &[u8],
        width: i32,
        height: i32,
        wrap_s: u32,
    ) -> Option<glow::Texture> {
        unsafe {
            let Ok(texture) = gl.create_texture() else {
                eprintln!("[mapskit] texture creation failed");
                return None;
            };
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                width,
                height,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(rgba)),
            );
            gl.generate_mipmap(glow::TEXTURE_2D);
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR_MIPMAP_LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, wrap_s as i32);
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );
            Some(texture)
        }
    }

    /// Uploads RGBA8 bytes as the earth texture (must be called on the GL
    /// thread).
    fn upload_earth_texture(&mut self, gl: &Gl, rgba: &[u8], width: i32, height: i32) {
        unsafe {
            if let Some(old) = self.earth_texture.take() {
                gl.delete_texture(old);
            }
        }
        self.earth_texture =
            Self::upload_rgba_texture(gl, rgba, width, height, glow::REPEAT);
    }

    /// Uploads RGBA8 bytes as the regional detail texture together with the
    /// area it covers (must be called on the GL thread).
    fn upload_detail_texture(
        &mut self,
        gl: &Gl,
        rgba: &[u8],
        width: i32,
        height: i32,
        region: [f32; 4],
    ) {
        unsafe {
            if let Some((old, _)) = self.detail_texture.take() {
                gl.delete_texture(old);
            }
        }
        let texture = Self::upload_rgba_texture(gl, rgba, width, height, glow::CLAMP_TO_EDGE);
        self.detail_texture = texture.map(|t| (t, region));
    }

    /// Drops the regional detail texture (must be called on the GL thread).
    fn clear_detail_texture(&mut self, gl: &Gl) {
        if let Some((old, _)) = self.detail_texture.take() {
            unsafe { gl.delete_texture(old) };
        }
    }
}

// ═══════════════════════════════════════════════════════════════
// State
// ═══════════════════════════════════════════════════════════════

struct GlobeState {
    config: MapsConfiguration,
    chain: Arc<ProviderChain>,
    center_lat: f64,
    center_lon: f64,
    distance: f64,
    auto_rotate: bool,
    dragging: bool,
    /// Center coordinates captured at drag start, so the cumulative drag
    /// offset maps to an absolute rotation (GTK reports offsets, not deltas).
    drag_start_center: Option<(f64, f64)>,
    markers: Vec<GlobeMarker>,
    texture_inbox: Rc<RefCell<Option<(Vec<u8>, i32, i32)>>>,
    texture_requested: bool,
    /// Zoom level of the detail texture currently uploaded (0 = none).
    detail_zoom: u8,
    /// Bounds of the uploaded detail texture
    /// `(lon_min, lon_max, merc_y_top, merc_y_bottom)` in radians.
    detail_region: [f32; 4],
    /// Zoom level of the in-flight detail stitch request, if any.
    detail_pending: Option<u8>,
    /// Finished detail stitch waiting for adoption on the GL thread.
    detail_inbox: Rc<RefCell<Option<(Vec<u8>, i32, i32, [f32; 4])>>>,
    /// Set when the detail texture must be dropped on the GL thread.
    clear_detail: bool,
}

/// The tile zoom level worth fetching for a camera distance. 0 = base
/// whole-world texture is enough; each step doubles the ground resolution.
fn desired_detail_zoom(distance: f64) -> u8 {
    match distance {
        d if d < 1.5 => 7,
        d if d < 1.75 => 6,
        d if d < 2.1 => 5,
        d if d < 2.9 => 4,
        _ => 0,
    }
}

impl GlobeState {
    fn model_matrix(&self) -> Mat4 {
        // RotX(center_lat) * RotY(-center_lon): brings the coordinate facing
        // the viewer onto the +Z axis towards the camera.
        let rot_x = mat_rotation_x((self.center_lat as f32).to_radians());
        let rot_y = mat_rotation_y(-(self.center_lon as f32).to_radians());
        mat_multiply(&rot_x, &rot_y)
    }

    fn mvp(&self, aspect: f32) -> Mat4 {
        let dist = self.distance as f32;
        let projection = mat_perspective(45.0_f32.to_radians(), aspect, 0.1, 30.0);
        let view = mat_translation_z(-dist);
        let mv = mat_multiply(&view, &self.model_matrix());
        mat_multiply(&projection, &mv)
    }

    /// Clamps latitude, wraps longitude and bounds the camera distance.
    fn clamp_center(&mut self) {
        self.center_lat = self.center_lat.clamp(-89.0, 89.0);
        if !(-180.0..=180.0).contains(&self.center_lon) {
            self.center_lon = ((self.center_lon + 180.0).rem_euclid(360.0)) - 180.0;
        }
        self.distance = self.distance.clamp(MIN_DISTANCE, MAX_DISTANCE);
    }
}

// ═══════════════════════════════════════════════════════════════
// Rendering
// ═══════════════════════════════════════════════════════════════

fn create_gl_resources() -> Result<(Gl, GlResources), String> {
    let get_proc = crate::ffi::get_gl_proc_address().ok_or("no GL proc address")?;
    let gl = unsafe {
        glow::Context::from_loader_function_cstr(|name| {
            get_proc(name.as_ptr()) as *const std::os::raw::c_void
        })
    };
    let resources = GlResources::new(&gl)?;
    Ok((gl, resources))
}

fn draw_frame(
    area: &GLArea,
    gl: &Gl,
    resources: &mut GlResources,
    state: &Rc<RefCell<GlobeState>>,
) {
    let s = state.borrow();

    let alloc = area.allocation();
    let scale = area.scale_factor() as i32;
    let width = (alloc.width() * scale).max(1);
    let height = (alloc.height() * scale).max(1);
    let aspect = width as f32 / height as f32;

    let mvp = s.mvp(aspect);
    let model = s.model_matrix();

    unsafe {
        gl.viewport(0, 0, width, height);
        gl.clear_color(0.05, 0.07, 0.11, 1.0);
        gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
        gl.enable(glow::DEPTH_TEST);
        gl.enable(glow::CULL_FACE);
        gl.cull_face(glow::BACK);
        gl.disable(glow::BLEND);

        gl.use_program(Some(resources.globe_program));
        if let Some(loc) = &resources.uniforms.globe_mvp {
            gl.uniform_matrix_4_f32_slice(Some(loc), false, &mvp);
        }
        if let Some(loc) = &resources.uniforms.globe_model {
            gl.uniform_matrix_4_f32_slice(Some(loc), false, &model);
        }
        if let Some(loc) = &resources.uniforms.globe_light {
            gl.uniform_3_f32(Some(loc), -0.55, 0.35, 0.75);
        }
        let has_texture = resources.earth_texture.is_some();
        if let Some(loc) = &resources.uniforms.has_texture {
            gl.uniform_1_i32(Some(loc), has_texture as i32);
        }
        if let Some(loc) = &resources.uniforms.earth_sampler {
            if let Some(texture) = resources.earth_texture {
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.uniform_1_i32(Some(loc), 0);
            }
        }

        // Regional detail layer on texture unit 1.
        let has_detail = resources.detail_texture.is_some();
        if let Some(loc) = &resources.uniforms.has_detail {
            gl.uniform_1_i32(Some(loc), has_detail as i32);
        }
        if let Some(loc) = &resources.uniforms.detail_sampler {
            gl.uniform_1_i32(Some(loc), 1);
        }
        if let Some(loc) = &resources.uniforms.detail_region {
            if let Some((_, region)) = resources.detail_texture {
                gl.uniform_4_f32(Some(loc), region[0], region[1], region[2], region[3]);
            }
        }
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(
            glow::TEXTURE_2D,
            resources.detail_texture.map(|(t, _)| t),
        );
        gl.active_texture(glow::TEXTURE0);

        gl.bind_vertex_array(Some(resources.sphere_vao));
        gl.draw_elements(
            glow::TRIANGLES,
            resources.index_count,
            glow::UNSIGNED_INT,
            0,
        );

        // Markers: accent dots on the front hemisphere, no depth write so
        // they stay visible at the silhouette.
        if !s.markers.is_empty() {
            gl.disable(glow::DEPTH_TEST);
            gl.enable(glow::BLEND);
            gl.blend_func_separate(
                glow::SRC_ALPHA,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE,
            );

            let mut points: Vec<[f32; 3]> = Vec::with_capacity(MAX_MARKERS);
            for marker in s.markers.iter().take(MAX_MARKERS) {
                points.push(sphere_point(marker.coordinate.latitude, marker.coordinate.longitude));
            }
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(resources.marker_vbo));
            gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, as_bytes(&points));

            gl.use_program(Some(resources.marker_program));
            if let Some(loc) = &resources.uniforms.marker_mvp {
                gl.uniform_matrix_4_f32_slice(Some(loc), false, &mvp);
            }
            if let Some(loc) = &resources.uniforms.marker_model {
                gl.uniform_matrix_4_f32_slice(Some(loc), false, &model);
            }
            if let Some(loc) = &resources.uniforms.marker_size {
                gl.uniform_1_f32(Some(loc), 8.0 * scale as f32);
            }
            if let Some(loc) = &resources.uniforms.marker_color {
                gl.uniform_3_f32(Some(loc), 1.0, 0.42, 0.17);
            }

            gl.bind_vertex_array(Some(resources.marker_vao));
            gl.draw_arrays(glow::POINTS, 0, points.len() as i32);
        }

        gl.bind_vertex_array(None);
    }
}

// ═══════════════════════════════════════════════════════════════
// GlobeView
// ═══════════════════════════════════════════════════════════════

/// The interactive 3D globe view.
pub struct GlobeView {
    area: GLArea,
    state: Rc<RefCell<GlobeState>>,
}

impl GlobeView {
    /// Creates a globe view. When `config.globe_earth_texture` is set, the
    /// earth texture download starts immediately.
    pub fn new(config: &MapsConfiguration) -> Self {
        let state = Rc::new(RefCell::new(GlobeState {
            config: config.clone(),
            chain: Arc::new(ProviderChain::with_config(config)),
            center_lat: 25.0,
            center_lon: 10.0,
            distance: 3.0,
            auto_rotate: true,
            dragging: false,
            drag_start_center: None,
            markers: Vec::new(),
            texture_inbox: Rc::new(RefCell::new(None)),
            texture_requested: false,
            detail_zoom: 0,
            detail_region: [0.0; 4],
            detail_pending: None,
            detail_inbox: Rc::new(RefCell::new(None)),
            clear_detail: false,
        }));

        let area = GLArea::new();
        area.set_hexpand(true);
        area.set_vexpand(true);
        area.set_required_version(3, 3);
        area.set_auto_render(true);

        let view = Self {
            area: area.clone(),
            state,
        };
        view.setup_gl(&area);
        view.setup_gestures(&area);
        view.setup_timer(&area);

        if config.globe_earth_texture {
            view.request_earth_texture();
        }

        view
    }

    /// The underlying GTK widget for embedding.
    pub fn widget(&self) -> &GLArea {
        &self.area
    }

    fn setup_gl(&self, area: &GLArea) {
        let state = self.state.clone();
        let session: Rc<RefCell<Option<(Gl, GlResources)>>> = Rc::new(RefCell::new(None));
        let session_clone = session.clone();

        area.connect_render(move |area, _context| {
            if session_clone.borrow().is_none() {
                match create_gl_resources() {
                    Ok(created) => *session_clone.borrow_mut() = Some(created),
                    Err(e) => {
                        eprintln!("[mapskit] GL init failed: {e}");
                        return glib::Propagation::Stop;
                    }
                }
            }

            // Adopt a freshly stitched earth texture when one arrived.
            let inbox_content = state.borrow_mut().texture_inbox.borrow_mut().take();
            if let Some((rgba, w, h)) = inbox_content {
                let mut session = session_clone.borrow_mut();
                if let Some((gl, resources)) = session.as_mut() {
                    resources.upload_earth_texture(gl, &rgba, w, h);
                }
            }

            // Drop or adopt the regional detail texture.
            {
                let mut s = state.borrow_mut();
                if s.clear_detail {
                    s.clear_detail = false;
                    let mut session = session_clone.borrow_mut();
                    if let Some((gl, resources)) = session.as_mut() {
                        resources.clear_detail_texture(gl);
                    }
                }
            }
            let detail_content = state.borrow_mut().detail_inbox.borrow_mut().take();
            if let Some((rgba, w, h, region)) = detail_content {
                let mut session = session_clone.borrow_mut();
                if let Some((gl, resources)) = session.as_mut() {
                    resources.upload_detail_texture(gl, &rgba, w, h, region);
                }
            }

            let mut session = session_clone.borrow_mut();
            let Some((gl, resources)) = session.as_mut() else {
                return glib::Propagation::Stop;
            };
            draw_frame(area, gl, resources, &state);
            glib::Propagation::Proceed
        });
    }

    fn setup_gestures(&self, area: &GLArea) {
        const ROTATION_DEGREES_PER_PIXEL: f64 = 0.25;

        let drag = GestureDrag::new();
        let state_drag = self.state.clone();
        drag.connect_drag_begin(move |_drag, _x, _y| {
            let mut s = state_drag.borrow_mut();
            s.dragging = true;
            s.drag_start_center = Some((s.center_lat, s.center_lon));
        });
        let state_update = self.state.clone();
        let area_update = area.clone();
        drag.connect_drag_update(move |_drag, dx, dy| {
            {
                let mut s = state_update.borrow_mut();
                // GTK reports the cumulative offset from the drag start; map
                // it to an absolute rotation instead of accumulating deltas.
                if let Some((start_lat, start_lon)) = s.drag_start_center {
                    s.center_lon = start_lon - dx * ROTATION_DEGREES_PER_PIXEL;
                    s.center_lat = start_lat + dy * ROTATION_DEGREES_PER_PIXEL;
                    s.clamp_center();
                }
            }
            area_update.queue_draw();
        });
        let state_end = self.state.clone();
        drag.connect_drag_end(move |_drag, _x, _y| {
            let mut s = state_end.borrow_mut();
            s.dragging = false;
            s.drag_start_center = None;
        });
        area.add_controller(drag);

        let scroll = EventControllerScroll::new(
            gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
        );
        let state_scroll = self.state.clone();
        let area_scroll = area.clone();
        scroll.connect_scroll(move |_, _dx, dy| {
            {
                let mut s = state_scroll.borrow_mut();
                s.distance += if dy < 0.0 { -0.25 } else { 0.25 };
                s.clamp_center();
            }
            area_scroll.queue_draw();
            glib::Propagation::Proceed
        });
        area.add_controller(scroll);
    }

    fn setup_timer(&self, area: &GLArea) {
        // Self-cancelling timer holding only a weak reference, following the
        // ShaderView pattern: stops itself once the widget is destroyed.
        let state_weak = self.state.clone();
        let area_weak = area.downgrade();
        glib::timeout_add_local(std::time::Duration::from_millis(33), move || {
            let Some(area) = area_weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let mut redraw;
            {
                let mut s = state_weak.borrow_mut();
                let should_rotate = s.auto_rotate && !s.dragging;
                if should_rotate {
                    s.center_lon -= 0.18;
                    s.clamp_center();
                }
                redraw = should_rotate;
            }
            redraw |= update_detail_lod(&state_weak, &area);
            if redraw {
                area.queue_draw();
            }
            glib::ControlFlow::Continue
        });
    }

    // ─── Camera ────────────────────────────────────────────

    /// Centers the globe on a coordinate.
    pub fn set_center(&self, coordinate: Coordinate) {
        {
            let mut s = self.state.borrow_mut();
            s.center_lat = coordinate.latitude;
            s.center_lon = coordinate.longitude;
            s.clamp_center();
        }
        self.area.queue_draw();
    }

    /// The coordinate currently facing the viewer.
    pub fn center(&self) -> Coordinate {
        let s = self.state.borrow();
        Coordinate::new(s.center_lat, s.center_lon)
    }

    /// Sets the camera distance (smaller = closer).
    pub fn set_distance(&self, distance: f64) {
        {
            let mut s = self.state.borrow_mut();
            s.distance = distance;
            s.clamp_center();
        }
        self.area.queue_draw();
    }

    /// The camera distance.
    pub fn distance(&self) -> f64 {
        self.state.borrow().distance
    }

    // ─── Rotation ──────────────────────────────────────────

    /// Enables or disables the idle spin.
    pub fn set_auto_rotate(&self, enabled: bool) {
        self.state.borrow_mut().auto_rotate = enabled;
    }

    /// Whether idle spin is enabled.
    pub fn auto_rotate(&self) -> bool {
        self.state.borrow().auto_rotate
    }

    // ─── Markers ───────────────────────────────────────────

    /// Replaces all markers.
    pub fn set_markers(&self, markers: Vec<GlobeMarker>) {
        self.state.borrow_mut().markers = markers;
        self.area.queue_draw();
    }

    /// Adds one marker. At most [`MAX_MARKERS`] markers are kept.
    pub fn add_marker(&self, marker: GlobeMarker) {
        {
            let mut s = self.state.borrow_mut();
            if s.markers.len() < MAX_MARKERS {
                s.markers.push(marker);
            }
        }
        self.area.queue_draw();
    }

    /// Removes all markers.
    pub fn clear_markers(&self) {
        self.state.borrow_mut().markers.clear();
        self.area.queue_draw();
    }

    /// All current markers.
    pub fn markers(&self) -> Vec<GlobeMarker> {
        self.state.borrow().markers.clone()
    }

    // ─── Texture ───────────────────────────────────────────

    /// Starts fetching a fresh earth texture from the providers.
    pub fn reload_earth_texture(&self) {
        self.request_earth_texture();
    }

    fn request_earth_texture(&self) {
        {
            let mut s = self.state.borrow_mut();
            if s.texture_requested {
                return;
            }
            s.texture_requested = true;
        }

        let state = self.state.clone();
        let chain = state.borrow().chain.clone();
        let config = state.borrow().config.clone();
        let (sender, receiver) = async_channel::unbounded::<Option<(Vec<u8>, i32, i32)>>();

        // ProviderChain is Send + Sync (Arc of Send+Sync providers).
        std::thread::spawn(move || {
            let result = stitch_earth_texture(&config, &chain);
            let _ = sender.send_blocking(result);
        });

        let area = self.area.clone();
        glib::spawn_future_local(async move {
            while let Ok(result) = receiver.recv().await {
                {
                    let mut s = state.borrow_mut();
                    s.texture_requested = false;
                    if let Some(texture) = result {
                        *s.texture_inbox.borrow_mut() = Some(texture);
                    }
                }
                area.queue_draw();
            }
        });
    }
}

/// Stitches a 4x4 grid of zoom-2 tiles (1024x1024 px) into an RGBA image.
///
/// Runs on a worker thread. Returns `None` when any tile fails; the globe
/// then keeps its procedural fallback shading.
fn stitch_earth_texture(
    config: &MapsConfiguration,
    chain: &ProviderChain,
) -> Option<(Vec<u8>, i32, i32)> {
    stitch_region(config, chain, 2, Coordinate::new(25.0, 10.0))
        .map(|(packed, width, height, _region)| (packed, width, height))
}

/// Geographic bounds of a stitched tile grid as
/// `(west_lon_deg, east_lon_deg, north_merc_y, south_merc_y)` plus the
/// top-left tile. `merc_y` is the Web-Mercator ordinate in radians.
fn region_bounds(zoom: u8, grid: u32, fx: f64, fy: f64) -> ([f64; 4], u32, u32) {
    let n = f64::from(1u32 << zoom);
    let grid = grid.min(n as u32);
    let half = f64::from(grid / 2);
    let max_offset = (n as i64 - i64::from(grid)).max(0);
    let x0 = ((fx.round() - half).round() as i64).clamp(0, max_offset) as u32;
    let y0 = ((fy.round() - half).round() as i64).clamp(0, max_offset) as u32;
    let west = x0 as f64 / n * 360.0 - 180.0;
    let east = (x0 + grid) as f64 / n * 360.0 - 180.0;
    let north_my = std::f64::consts::PI * (1.0 - 2.0 * y0 as f64 / n);
    let south_my = std::f64::consts::PI * (1.0 - 2.0 * (y0 + grid) as f64 / n);
    ([west, east, north_my, south_my], x0, y0)
}

/// Stitches a `DETAIL_GRID` x `DETAIL_GRID` tile block at `zoom` centered on
/// `center` into an RGBA image and reports the area it covers in radians
/// `(lon_min, lon_max, merc_y_top, merc_y_bottom)`.
///
/// Runs on a worker thread. Returns `None` when any tile fails.
fn stitch_region(
    config: &MapsConfiguration,
    chain: &ProviderChain,
    zoom: u8,
    center: Coordinate,
) -> Option<(Vec<u8>, i32, i32, [f64; 4])> {
    const GRID: u32 = 4;
    const TILE_PX: i32 = 256;
    let size = GRID * TILE_PX as u32;

    let provider = chain.tile_provider_for(config.style).cloned()?;
    let client = reqwest::blocking::Client::builder()
        .user_agent(config.user_agent.clone())
        .timeout(std::time::Duration::from_secs(config.timeout_seconds.max(10)))
        .build()
        .ok()?;

    let (fx, fy) = crate::tiles::tile_for(center, zoom);
    let ([west, east, north_my, south_my], x0, y0) = region_bounds(zoom, GRID, fx, fy);

    let dest = gdk_pixbuf::Pixbuf::new(
        gdk_pixbuf::Colorspace::Rgb,
        true,
        8,
        size as i32,
        size as i32,
    )?;

    for gy in 0..GRID {
        for gx in 0..GRID {
            let url = provider.tile_url(x0 + gx, y0 + gy, zoom);
            let resp = client.get(&url).send().ok()?;
            if !resp.status().is_success() {
                return None;
            }
            let bytes = resp.bytes().ok()?;
            let loader = gdk_pixbuf::PixbufLoader::new();
            loader.write(&bytes).ok()?;
            loader.close().ok()?;
            let tile = loader.pixbuf()?;
            tile.composite(
                &dest,
                (gx * TILE_PX as u32) as i32,
                (gy * TILE_PX as u32) as i32,
                TILE_PX,
                TILE_PX,
                (gx * TILE_PX as u32) as f64,
                (gy * TILE_PX as u32) as f64,
                1.0,
                1.0,
                gdk_pixbuf::InterpType::Bilinear,
                255,
            );
        }
    }

    let width = dest.width();
    let height = dest.height();
    let rowstride = dest.rowstride() as usize;
    let channels = dest.n_channels() as usize;
    let raw = dest.pixel_bytes()?;

    let mut packed = Vec::with_capacity(width as usize * height as usize * 4);
    for row in 0..height as usize {
        let start = row * rowstride;
        let end = start + width as usize * channels;
        let row_bytes = raw.get(start..end)?;
        if channels == 4 {
            packed.extend_from_slice(row_bytes);
        } else {
            for chunk in row_bytes.chunks(channels) {
                packed.extend_from_slice(chunk);
                packed.push(255);
            }
        }
    }

    Some((
        packed,
        width,
        height,
        [
            west.to_radians(),
            east.to_radians(),
            north_my,
            south_my,
        ],
    ))
}

/// Evaluates the detail-texture LOD for the current camera distance and
/// kicks off a sharper regional stitch when needed. Returns whether the
/// view changed and needs a redraw.
fn update_detail_lod(state: &Rc<RefCell<GlobeState>>, area: &GLArea) -> bool {
    let want = {
        let s = state.borrow();
        desired_detail_zoom(s.distance)
    };

    // Zoomed back out far enough: drop the detail layer again.
    if want == 0 {
        let mut s = state.borrow_mut();
        if s.detail_zoom != 0 && s.detail_pending.is_none() {
            s.clear_detail = true;
            s.detail_zoom = 0;
            return true;
        }
        return false;
    }

    // Request a sharper stitch around the current center, or re-request the
    // loaded level when the center drifted out of its region.
    let (center, config, chain, request_zoom) = {
        let mut s = state.borrow_mut();
        if s.dragging || s.detail_pending.is_some() {
            return false;
        }
        // Same level already loaded and the center is still inside its
        // region: nothing to do.
        let request_zoom = want.max(s.detail_zoom);
        if request_zoom == 0
            || (want <= s.detail_zoom
                && !center_outside_region(s.center_lat, s.center_lon, &s.detail_region))
        {
            return false;
        }

        s.detail_pending = Some(request_zoom);
        (
            Coordinate::new(s.center_lat, s.center_lon),
            s.config.clone(),
            s.chain.clone(),
            request_zoom,
        )
    };

    let (sender, receiver) = async_channel::unbounded::<Option<(Vec<u8>, i32, i32, [f64; 4])>>();
    std::thread::spawn(move || {
        let result = stitch_region(&config, &chain, request_zoom, center);
        let _ = sender.send_blocking(result);
    });

    let state = state.clone();
    let area = area.clone();
    glib::spawn_future_local(async move {
        let Ok(result) = receiver.recv().await else {
            return;
        };
        let mut s = state.borrow_mut();
        s.detail_pending = None;
        if let Some((rgba, width, height, region)) = result {
            *s.detail_inbox.borrow_mut() =
                Some((rgba, width, height, region.map(|v| v as f32)));
            s.detail_region = region.map(|v| v as f32);
            s.detail_zoom = request_zoom;
            area.queue_draw();
        }
        // A failed fetch keeps the previous detail level; the next tick
        // retries once the pending slot is free again.
    });

    false
}

/// Whether a lat/lon point lies outside a detail region
/// `(lon_min, lon_max, merc_y_top, merc_y_bottom)`.
fn center_outside_region(latitude: f64, longitude: f64, region: &[f32; 4]) -> bool {
    let lat_clamped = latitude.clamp(-85.0511, 85.0511);
    let my = ((lat_clamped.to_radians() / 2.0 + std::f64::consts::FRAC_PI_4).tan()).ln();
    let lon = longitude.to_radians();
    lon < f64::from(region[0])
        || lon > f64::from(region[1])
        || my > f64::from(region[2])
        || my < f64::from(region[3])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sphere_mesh_size() {
        let (positions, indices) = build_sphere_mesh();
        assert_eq!(
            positions.len(),
            (SPHERE_LAT_BANDS + 1) * (SPHERE_LON_SEGS + 1)
        );
        assert_eq!(indices.len(), SPHERE_LAT_BANDS * SPHERE_LON_SEGS * 6);
        for p in positions.iter().step_by(97) {
            let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn sphere_point_axes() {
        let north = sphere_point(90.0, 0.0);
        assert!(north[1] > 0.999);

        let greenwich = sphere_point(0.0, 0.0);
        assert!(greenwich[2] > 0.999);

        let east = sphere_point(0.0, 90.0);
        assert!(east[0] > 0.999);
    }

    fn test_state(lat: f64, lon: f64) -> GlobeState {
        GlobeState {
            config: MapsConfiguration::new(),
            chain: Arc::new(ProviderChain::default_providers()),
            center_lat: lat,
            center_lon: lon,
            distance: 3.0,
            auto_rotate: false,
            dragging: false,
            drag_start_center: None,
            markers: Vec::new(),
            texture_inbox: Rc::new(RefCell::new(None)),
            texture_requested: false,
            detail_zoom: 0,
            detail_region: [0.0; 4],
            detail_pending: None,
            detail_inbox: Rc::new(RefCell::new(None)),
            clear_detail: false,
        }
    }

    #[test]
    fn detail_zoom_follows_distance() {
        assert_eq!(desired_detail_zoom(5.0), 0);
        assert_eq!(desired_detail_zoom(3.0), 0);
        assert_eq!(desired_detail_zoom(2.8), 4);
        assert_eq!(desired_detail_zoom(2.0), 5);
        assert_eq!(desired_detail_zoom(1.6), 6);
        assert_eq!(desired_detail_zoom(1.2), 7);
        // Monotonic while zooming in.
        let mut last = 0;
        for step in 0..100 {
            let d = MAX_DISTANCE - (MAX_DISTANCE - MIN_DISTANCE) * step as f64 / 99.0;
            let z = desired_detail_zoom(d);
            assert!(z >= last, "detail zoom decreased while zooming in");
            last = z;
        }
    }

    #[test]
    fn base_region_covers_whole_world() {
        let (region, x0, y0) = region_bounds(2, 4, 1.25, 0.75);
        let pi = std::f64::consts::PI;
        assert_eq!(x0, 0);
        assert_eq!(y0, 0);
        assert!((region[0] - -180.0).abs() < 1e-9);
        assert!((region[1] - 180.0).abs() < 1e-9);
        assert!((region[2] - pi).abs() < 1e-9);
        assert!((region[3] - -pi).abs() < 1e-9);
    }

    #[test]
    fn region_contains_requested_center() {
        // Munich fractional tile coords at zoom 5.
        let center = Coordinate::new(48.137, 11.575);
        let (fx, fy) = crate::tiles::tile_for(center, 5);
        let (region, _x0, _y0) = region_bounds(5, 4, fx, fy);
        let my = ((center.latitude.to_radians() / 2.0 + std::f64::consts::FRAC_PI_4).tan()).ln();
        let lon = center.longitude.to_radians();
        assert!(lon > region[0] && lon < region[1]);
        assert!(my < region[2] && my > region[3]);
        assert!(region[2] > region[3], "north above south");
    }

    #[test]
    fn model_matrix_faces_camera() {
        for (lat, lon) in [(0.0, 0.0), (45.0, 90.0), (-30.0, -120.0)] {
            let state = test_state(lat, lon);
            let m = state.model_matrix();
            let p = sphere_point(lat, lon);
            let mut out = [0.0_f32; 3];
            for row in 0..3 {
                out[row] = m[row] * p[0] + m[4 + row] * p[1] + m[8 + row] * p[2] + m[12 + row];
            }
            assert!(out[2] > 0.99, "lat={lat} lon={lon}: {out:?}");
            assert!(out[0].abs() < 0.01, "lat={lat} lon={lon}: {out:?}");
            assert!(out[1].abs() < 0.01, "lat={lat} lon={lon}: {out:?}");
        }
    }

    #[test]
    fn clamp_state() {
        let mut state = test_state(120.0, 400.0);
        state.distance = 50.0;
        state.clamp_center();
        assert_eq!(state.center_lat, 89.0);
        assert!((-180.0..=180.0).contains(&state.center_lon));
        assert!(state.distance <= 6.0);
    }

    #[test]
    fn mat_math_basics() {
        let identity = mat_identity();
        let rot = mat_rotation_y(1.0);
        assert_eq!(mat_multiply(&identity, &rot), rot);

        let persp = mat_perspective(45f32.to_radians(), 1.5, 0.1, 30.0);
        assert!(persp[11] < 0.0, "perspective uses -1 in the w row");

        let translated = mat_translation_z(-3.0);
        assert_eq!(translated[14], -3.0);
    }
}
