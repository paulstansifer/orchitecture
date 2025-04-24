extends Node3D

var map: DataMap


@export var drag_start_helper: Node3D
@export var mouse_helper: Node3D # The 'cursor'
@export var selector_container: Node3D # Node that holds a preview of the structure
@export var view_camera: Camera3D # Used for raycasting mouse
@export var gridmap: GridMap
@export var cash_display: Label
@export var wallgrid: WallGrid

@onready var buildable_buttons: Control = get_node("../CanvasLayer/Control/BuildableButtons")

var wall_meshes: Array[Structure]
var selected_mesh_id: int = 0
var cur_y: int = 0 # The current layer to interact with
var plane: Plane # Used for raycasting mouse
var drag_start: Vector3

func _ready():
	map = DataMap.new()
	plane = Plane(Vector3.UP, Vector3.ZERO)

	# WallGrid MeshLibrary
	var wallgrid_mesh_library = MeshLibrary.new()

	var dir_access = DirAccess.open("res://buildables")
	if dir_access != null:
		dir_access.list_dir_begin()
		var file_name = dir_access.get_next()
		while file_name != "":
			if file_name.ends_with(".gltf"):
				var model = ResourceLoader.load("res://buildables/" + file_name) as PackedScene
				if model != null:
					var buildable_resource = Structure.new()
					var id = wallgrid_mesh_library.get_last_unused_item_id()
					wallgrid_mesh_library.create_item(id)
					wallgrid_mesh_library.set_item_mesh(id, get_mesh(model))
					wallgrid_mesh_library.set_item_mesh_transform(id, Transform3D().rotated(Vector3.RIGHT, -TAU / 4).translated(Vector3(-.5, -.5, .5)))
					buildable_resource.id = id
					buildable_resource.name = file_name
					wall_meshes.append(buildable_resource)
			file_name = dir_access.get_next()
		dir_access.list_dir_end()

	wallgrid.set_mesh_library(wallgrid_mesh_library)

	# Create buttons for each buildable
	for buildable in wall_meshes:
		var button = Button.new()

		button.text = buildable.name

		button.connect("pressed", _on_buildable_button_pressed.bind(buildable.id))
		buildable_buttons.add_child(button)

	load_map()

func _process(_delta):
	# Controls
	accept_actions()

	if Input.is_action_just_pressed("y_layer_up"):
		cur_y += 1
		if cur_y > 10:
			cur_y = 10
		plane.d = cur_y

	if Input.is_action_just_pressed("y_layer_down"):
		cur_y -= 1
		if cur_y < 0:
			cur_y = 0
		plane.d = cur_y
	
	# Map position based on mouse
	
	var world_position = plane.intersects_ray(
		view_camera.project_ray_origin(get_viewport().get_mouse_position()),
		view_camera.project_ray_normal(get_viewport().get_mouse_position()))

	if not world_position:
		return

	mouse_helper.position = world_position.round()

	if Input.is_action_just_pressed("build"):
		drag_start_helper.position = world_position.round()
		drag_start_helper.show()
		drag_start = world_position

	if Input.is_action_just_released("build"):
		drag_start_helper.hide()
		self.wallgrid.paint_wall(drag_start, world_position, selected_mesh_id)



# Retrieve the mesh from a PackedScene, used for dynamically creating a MeshLibrary

func get_mesh(packed_scene):
	var scene_state: SceneState = packed_scene.get_state()
	for i in range(scene_state.get_node_count()):
		if (scene_state.get_node_type(i) == "MeshInstance3D"):
			for j in scene_state.get_node_property_count(i):
				var prop_name = scene_state.get_node_property_name(i, j)
				if prop_name == "mesh":
					var prop_value = scene_state.get_node_property_value(i, j)
					
					return prop_value.duplicate()


# Saving/load

func load_map():
	print("Loading map...")
	#TODO

func save_map():
	print("Saving map...")
	#TODO:

func accept_actions():
	if Input.is_action_just_pressed("save"):
		save_map()
	if Input.is_action_just_pressed("load"):
		load_map()

	if Input.is_action_just_pressed("quit"):
		get_tree().quit()

func _on_buildable_button_pressed(id):
	print("Button pressed! ID: ", id)
	selected_mesh_id = id
