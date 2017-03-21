#[macro_use]
extern crate gfx;
extern crate gfx_window_glutin;
extern crate gfx_device_gl;
extern crate glutin;
extern crate lyon;
extern crate lyon_renderer;

use lyon::extra::rust_logo::build_logo_path;
use lyon::path_builder::*;
use lyon::math::*;
use lyon::tessellation::geometry_builder::{ VertexConstructor, VertexBuffers, BuffersBuilder };
use lyon::tessellation::basic_shapes::*;
use lyon::tessellation::path_fill::{ FillTessellator, FillOptions };
use lyon::tessellation::path_stroke::{ StrokeTessellator, StrokeOptions };
use lyon::tessellation;
use lyon::path::Path;
use lyon::path_iterator::PathIterator;
use lyon_renderer::buffer::{ Id };
use lyon_renderer::shaders::*;
use lyon_renderer::renderer::{
    GpuBufferStore, PrimData, GpuTransform, Vertex,
    opaque_pipeline, transparent_pipeline, Globals, WithPrimitiveId,
    GpuGeometry,
};
use lyon_renderer::prim_store::{ BufferStore };
// make  public so that the module in gfx_defines can see the types.
pub use lyon_renderer::gfx_types::*;

use gfx::traits::FactoryExt;

use std::ops::Rem;

type FillVertex = Vertex;
type StrokeVertex = Vertex;

//type OpaquePso = Pso<opaque_pipeline::Meta>;
//type TransparentPso = Pso<transparent_pipeline::Meta>;

gfx_defines!{
    // The background is drawn separately with its own shader.
    vertex BgVertex {
        position: [f32; 2] = "a_position",
    }

    pipeline bg_pipeline {
        vbo: gfx::VertexBuffer<BgVertex> = (),
        out_color: gfx::RenderTarget<ColorFormat> = "out_color",
        out_depth: gfx::DepthTarget<DepthFormat> = gfx::preset::depth::LESS_EQUAL_WRITE,
        constants: gfx::ConstantBuffer<Globals> = "Globals",
    }
}

pub type TransformId = Id<GpuTransform>;

pub fn split_gfx_slice<R:gfx::Resources>(slice: gfx::Slice<R>, at: u32) -> (gfx::Slice<R>, gfx::Slice<R>) {
    let mut first = slice.clone();
    let mut second = slice.clone();
    first.end = at;
    second.start = at;

    return (first, second);
}

pub fn gfx_sub_slice<R:gfx::Resources>(slice: gfx::Slice<R>, from: u32, to: u32) -> gfx::Slice<R> {
    let mut sub = slice.clone();
    sub.start = from;
    sub.end = to;

    return sub;
}

struct BgWithPrimId ;
impl VertexConstructor<tessellation::FillVertex, BgVertex> for BgWithPrimId  {
    fn new_vertex(&mut self, vertex: tessellation::FillVertex) -> BgVertex {
        BgVertex { position: vertex.position.array() }
    }
}

struct Cpu {
    transforms: BufferStore<GpuTransform>,
    primitives: BufferStore<PrimData>,
    fills: VertexBuffers<FillVertex>,
    strokes: VertexBuffers<StrokeVertex>,
}

struct Gpu {
    transforms: GpuBufferStore<GpuTransform>,
    primitives: GpuBufferStore<PrimData>,
    //fills: GpuGeometry<FillVertex>,
    //strokes: GpuGeometry<StrokeVertex>,
}

fn main() {
    println!("== gfx-rs example ==");
    println!("Controls:");
    println!("  Arrow keys: scrolling");
    println!("  PgUp/PgDown: zoom in/out");
    println!("  w: toggle wireframe mode");
    println!("  p: toggle show points");
    println!("  a/z: increase/decrease the stroke width");

    let num_instances = 32;

    // Build a Path for the rust logo.
    let mut builder = SvgPathBuilder::new(Path::builder());
    build_logo_path(&mut builder);
    let path = builder.build();

    // Create some CPU-side buffers that will contain the geometry.
    let mut cpu = Cpu {
        fills: VertexBuffers::new(),
        strokes: VertexBuffers::new(),
        transforms: BufferStore::new(1, PRIM_BUFFER_LEN as u16),
        primitives: BufferStore::new(1, PRIM_BUFFER_LEN as u16),
    };

    let default_transform = cpu.transforms.push(GpuTransform::default());
    let logo_transforms = cpu.transforms.alloc_range(num_instances);

    // Tessellate the fill
    let logo_fill_ids = cpu.primitives.alloc_range(num_instances);

    // Note that we flatten the path here. Since the flattening tolerance should
    // depend on the resolution/zoom it would make sense to re-tessellate when the
    // zoom level changes (not done here for simplicity).
    let fill_count = FillTessellator::new().tessellate_path(
        path.path_iter().flattened(0.09),
        &FillOptions::default(),
        &mut BuffersBuilder::new(&mut cpu.fills, WithPrimitiveId(logo_fill_ids.range.start()))
    ).unwrap();

    cpu.primitives[logo_fill_ids.first()] = PrimData::new([1.0, 1.0, 1.0, 1.0], 0.1, logo_transforms.range.start());
    for i in 1..num_instances {
        cpu.primitives[logo_fill_ids.get(i)] = PrimData::new(
            [(0.1 * i as f32).rem(1.0), (0.5 * i as f32).rem(1.0), (0.9 * i as f32).rem(1.0), 1.0],
            0.1 - 0.001 * i as f32,
            logo_transforms.range.get(i)
        );
    }

    // Tessellate the stroke
    let logo_stroke_id = cpu.primitives.push(PrimData::new([0.0, 0.0, 0.0, 0.1], 0.2, default_transform.element));

    StrokeTessellator::new().tessellate(
        path.path_iter().flattened(0.022),
        &StrokeOptions::default(),
        &mut BuffersBuilder::new(&mut cpu.strokes, WithPrimitiveId(logo_stroke_id.element))
    ).unwrap();

    let mut num_points = 0;
    for p in path.as_slice().iter() {
        if p.destination().is_some() {
            num_points += 1;
        }
    }

    let point_transforms = cpu.transforms.alloc_range(num_points);
    let point_ids_1 =  cpu.primitives.alloc_range(num_points);
    let point_ids_2 =  cpu.primitives.alloc_range(num_points);

    let ellipse_indices_start =  cpu.fills.indices.len() as u32;
    let ellipsis_count = fill_ellipse(
        vec2(0.0, 0.0), vec2(1.0, 1.0), 64,
        &mut BuffersBuilder::new(&mut  cpu.fills, WithPrimitiveId(point_ids_1.range.start()))
    );
    fill_ellipse(
        vec2(0.0, 0.0), vec2(0.5, 0.5), 64,
        &mut BuffersBuilder::new(&mut  cpu.fills, WithPrimitiveId(point_ids_2.range.start()))
    );

    let mut i = 0;
    for evt in path.as_slice().iter() {
        if let Some(to) = evt.destination() {
            let transform_id = point_transforms.range.get(i);
            cpu.transforms[point_transforms.get(i)].transform = Mat4::create_translation(
                to.x, to.y, 0.0
            ).to_row_arrays();
            cpu.primitives[point_ids_1.get(i)] = PrimData::new(
                [0.0, 0.2, 0.0, 1.0],
                0.3,
                transform_id
            );
            cpu.primitives[point_ids_2.get(i)] = PrimData::new(
                [0.0, 1.0, 0.0, 1.0],
                0.4,
                transform_id
            );
            i += 1;
        }
    }

    println!(" -- fill: {} vertices {} indices",  cpu.fills.vertices.len(),  cpu.fills.indices.len());
    println!(" -- stroke: {} vertices {} indices",  cpu.strokes.vertices.len(),  cpu.strokes.indices.len());

    let mut bg_mesh_cpu: VertexBuffers<BgVertex> = VertexBuffers::new();
    fill_rectangle(
        &Rect::new(vec2(-1.0, -1.0), size(2.0, 2.0)),
        &mut BuffersBuilder::new(&mut bg_mesh_cpu, BgWithPrimId )
    );

    // Initialize glutin and gfx-rs (refer to gfx-rs examples for more details).

    let glutin_builder = glutin::WindowBuilder::new()
        .with_dimensions(700, 700)
        .with_decorations(true)
        .with_title("tessellation".to_string())
        .with_multisampling(8)
        .with_vsync();

    let (window, mut device, mut factory, mut main_fbo, mut main_depth) =
        gfx_window_glutin::init::<ColorFormat, DepthFormat>(glutin_builder);

    let constants = factory.create_constant_buffer(1);

    let mut gpu = Gpu {
        //fills: GpuGeometry::new(),
        //strokes: GpuGeometry::new(),
        transforms: GpuBufferStore::new(gfx::buffer::Role::Constant, gfx::memory::Usage::Dynamic),
        primitives: GpuBufferStore::new(gfx::buffer::Role::Constant, gfx::memory::Usage::Dynamic),
    };

    let bg_pso = factory.create_pipeline_simple(
        BACKGROUND_VERTEX_SHADER.as_bytes(),
        BACKGROUND_FRAGMENT_SHADER.as_bytes(),
        bg_pipeline::new()
    ).unwrap();

    let (bg_vbo, bg_range) = factory.create_vertex_buffer_with_slice(
        &bg_mesh_cpu.vertices[..],
        &bg_mesh_cpu.indices[..]
    );

    let model_shader = factory.link_program(
        FILL_VERTEX_SHADER.as_bytes(),
        FILL_FRAGMENT_SHADER.as_bytes(),
    ).unwrap();

    let opaque_pso = factory.create_pipeline_from_program(
        &model_shader,
        gfx::Primitive::TriangleList,
        gfx::state::Rasterizer::new_fill(),
        opaque_pipeline::new()
    ).unwrap();

    let _transparent_pso = factory.create_pipeline_from_program(
        &model_shader,
        gfx::Primitive::TriangleList,
        gfx::state::Rasterizer::new_fill(),
        transparent_pipeline::new()
    ).unwrap();

    let mut fill_mode = gfx::state::Rasterizer::new_fill();
    fill_mode.method = gfx::state::RasterMethod::Line(1);
    let wireframe_opaque_pso = factory.create_pipeline_from_program(
        &model_shader,
        gfx::Primitive::TriangleList,
        fill_mode,
        opaque_pipeline::new()
    ).unwrap();

    let _wireframe_transparent_pso = factory.create_pipeline_from_program(
        &model_shader,
        gfx::Primitive::TriangleList,
        fill_mode,
        transparent_pipeline::new()
    ).unwrap();

    let mut init_queue: gfx::Encoder<_, _> = factory.create_command_buffer().into();

    let (vbo, ibo) = factory.create_vertex_buffer_with_slice(
        &cpu.fills.vertices[..],
        &cpu.fills.indices[..],
    );
    let gpu_fills = GpuGeometry { vbo: vbo, ibo: ibo };

    let (vbo, ibo) = factory.create_vertex_buffer_with_slice(
        &cpu.strokes.vertices[..],
        &cpu.strokes.indices[..],
    );
    let gpu_strokes = GpuGeometry { vbo: vbo, ibo: ibo };

    //gpu.fills.update(&mut cpu.fills, &mut factory, &mut init_queue);
    //gpu.strokes.update(&mut cpu.strokes, &mut factory, &mut init_queue);
    gpu.primitives.update(&mut cpu.primitives, &mut factory, &mut init_queue);
    gpu.transforms.update(&mut cpu.transforms, &mut factory, &mut init_queue);
    init_queue.flush(&mut device);

    let split = ellipse_indices_start + (ellipsis_count.indices as u32);
    let mut points_range_1 = gfx_sub_slice(gpu_fills.ibo.clone(), ellipse_indices_start, split);
    let mut points_range_2 = gfx_sub_slice(gpu_fills.ibo.clone(), split, split + ellipsis_count.indices as u32);
    points_range_1.instances = Some((num_points as u32, 0));
    points_range_2.instances = Some((num_points as u32, 0));

    let mut fill_range = gfx_sub_slice(gpu_fills.ibo.clone(), 0, fill_count.indices as u32);
    fill_range.instances = Some((num_instances as u32, 0));

    let mut scene = SceneParams {
        target_zoom: 5.0,
        zoom: 0.5,
        target_scroll: vec2(70.0, 70.0),
        scroll: vec2(70.0, 70.0),
        show_points: false,
        show_wireframe: false,
        stroke_width: 0.0,
        target_stroke_width: 1.0,
    };

    let mut cmd_queue: gfx::Encoder<_, _> = factory.create_command_buffer().into();

    let mut frame_count: usize = 0;
    loop {
        if !update_inputs(&window, &mut scene) {
            break;
        }

        // Set the color of the second shape (the outline) to some slowly changing
        // pseudo-random color.
         cpu.primitives[logo_stroke_id].color = [
            (frame_count as f32 * 0.008 - 1.6).sin() * 0.1 + 0.1,
            (frame_count as f32 * 0.005 - 1.6).sin() * 0.1 + 0.1,
            (frame_count as f32 * 0.01 - 1.6).sin() * 0.1 + 0.1,
            1.0
        ];
        cpu.primitives[logo_stroke_id].width = scene.stroke_width;

        for i in 1..num_instances {
            cpu.transforms[logo_transforms.get(i)].transform = Mat4::create_translation(
                (frame_count as f32 * 0.001 * i as f32).sin() * (100.0 + i as f32 * 10.0),
                (frame_count as f32 * 0.002 * i as f32).sin() * (100.0 + i as f32 * 10.0),
                0.0
            ).to_row_arrays();
        }


        gfx_window_glutin::update_views(&window, &mut main_fbo, &mut main_depth);
        let (w, h) = window.get_inner_size_pixels().unwrap();

        cmd_queue.clear(&main_fbo.clone(), [0.0, 0.0, 0.0, 0.0]);
        cmd_queue.clear_depth(&main_depth.clone(), 1.0);

        gpu.primitives.update(&mut cpu.primitives, &mut factory, &mut cmd_queue);
        gpu.transforms.update(&mut cpu.transforms, &mut factory, &mut cmd_queue);

        cmd_queue.update_constant_buffer(&constants, &Globals {
            resolution: [w as f32, h as f32],
            zoom: scene.zoom,
            scroll_offset: scene.scroll.array(),
        });

        let default_pipeline_data = opaque_pipeline::Data {
            vbo: gpu_fills.vbo.clone(),
            out_color: main_fbo.clone(),
            out_depth: main_depth.clone(),
            constants: constants.clone(),
            prim_data: gpu.primitives[logo_fill_ids.buffer].clone(),
            transforms: gpu.transforms[logo_transforms.buffer].clone(),
        };

        // Draw the opaque geometry front to back with the depth buffer enabled.

        if scene.show_points {
            cmd_queue.draw(&points_range_1, &opaque_pso, &opaque_pipeline::Data {
                vbo: gpu_fills.vbo.clone(),
                .. default_pipeline_data.clone()
            });
            cmd_queue.draw(&points_range_2, &opaque_pso, &opaque_pipeline::Data {
                vbo: gpu_fills.vbo.clone(),
                .. default_pipeline_data.clone()
            });
        }

        let pso = if scene.show_wireframe { &wireframe_opaque_pso }
                  else { &opaque_pso };

        cmd_queue.draw(&fill_range, &pso, &opaque_pipeline::Data {
            vbo: gpu_fills.vbo.clone(),
            .. default_pipeline_data.clone()
        });

        cmd_queue.draw(&gpu_strokes.ibo, &pso, &opaque_pipeline::Data {
            vbo: gpu_strokes.vbo.clone(),
            .. default_pipeline_data.clone()
        });

        cmd_queue.draw(&bg_range, &bg_pso, &bg_pipeline::Data {
            vbo: bg_vbo.clone(),
            out_color: main_fbo.clone(),
            out_depth: main_depth.clone(),
            constants: constants.clone(),
        });

        //let pso = if scene.show_wireframe { &wireframe_transparent_pso }
        //          else { &transparent_pso };
        // Non-opaque geometry should be drawn back to front here.
        // (there is none in this example)


        cmd_queue.flush(&mut device);

        window.swap_buffers().unwrap();

        //device.cleanup(); // TODO

        frame_count += 1;
    }
}

struct SceneParams {
    target_zoom: f32,
    zoom: f32,
    target_scroll: Vec2,
    scroll: Vec2,
    show_points: bool,
    show_wireframe: bool,
    stroke_width: f32,
    target_stroke_width: f32,
}

fn update_inputs(window: &glutin::Window, scene: &mut SceneParams) -> bool {
    for event in window.poll_events() {
        use glutin::Event::KeyboardInput;
        use glutin::ElementState::Pressed;
        use glutin::VirtualKeyCode;
        match event {
            glutin::Event::Closed => {
                return false;
            }
            KeyboardInput(Pressed, _, Some(key)) => {
                match key {
                    VirtualKeyCode::Escape => {
                        return false;
                    }
                    VirtualKeyCode::PageDown => {
                        scene.target_zoom *= 0.8;
                    }
                    VirtualKeyCode::PageUp => {
                        scene.target_zoom *= 1.25;
                    }
                    VirtualKeyCode::Left => {
                        scene.target_scroll.x -= 50.0 / scene.target_zoom;
                    }
                    VirtualKeyCode::Right => {
                        scene.target_scroll.x += 50.0 / scene.target_zoom;
                    }
                    VirtualKeyCode::Up => {
                        scene.target_scroll.y -= 50.0 / scene.target_zoom;
                    }
                    VirtualKeyCode::Down => {
                        scene.target_scroll.y += 50.0 / scene.target_zoom;
                    }
                    VirtualKeyCode::P => {
                        scene.show_points = !scene.show_points;
                    }
                    VirtualKeyCode::W => {
                        scene.show_wireframe = !scene.show_wireframe;
                    }
                    VirtualKeyCode::A => {
                        scene.target_stroke_width += 0.8;
                    }
                    VirtualKeyCode::Z => {
                        scene.target_stroke_width -= 0.8;
                    }
                    _key => {}
                }
                println!(" -- zoom: {}, scroll: {:?}", scene.target_zoom, scene.target_scroll);
            }
            _evt => {
                //println!("{:?}", _evt);
            }
        };
    }

    scene.zoom += (scene.target_zoom - scene.zoom) / 3.0;
    scene.scroll = scene.scroll + (scene.target_scroll - scene.scroll) / 3.0;
    scene.stroke_width = scene.stroke_width + (scene.target_stroke_width - scene.stroke_width) / 5.0;

    return true;
}

static BACKGROUND_VERTEX_SHADER: &'static str = &"
    #version 140
    in vec2 a_position;
    out vec2 v_position;

    void main() {
        gl_Position = vec4(a_position, 1.0, 1.0);
        v_position = a_position;
    }
";

// The background.
// This shader is silly and slow, but it looks nice ;)
static BACKGROUND_FRAGMENT_SHADER: &'static str = &"
    #version 140
    uniform Globals {
        vec2 u_resolution;
        vec2 u_scroll_offset;
        float u_zoom;
    };
    in vec2 v_position;
    out vec4 out_color;

    void main() {
        vec2 px_position = v_position * vec2(1.0, -1.0) * u_resolution * 0.5;

        // #005fa4
        float vignette = clamp(0.0, 1.0, (0.7*length(v_position)));
        out_color = mix(
            vec4(0.0, 0.47, 0.9, 1.0),
            vec4(0.0, 0.1, 0.64, 1.0),
            vignette
        );

        // TODO: properly adapt the grid while zooming in and out.
        float grid_scale = 5.0;
        if (u_zoom < 2.5) {
            grid_scale = 1.0;
        }

        vec2 pos = px_position + u_scroll_offset * u_zoom;

        if (mod(pos.x, 20.0 / grid_scale * u_zoom) <= 1.0 ||
            mod(pos.y, 20.0 / grid_scale * u_zoom) <= 1.0) {
            out_color *= 1.2;
        }

        if (mod(pos.x, 100.0 / grid_scale * u_zoom) <= 2.0 ||
            mod(pos.y, 100.0 / grid_scale * u_zoom) <= 2.0) {
            out_color *= 1.2;
        }
    }
";
