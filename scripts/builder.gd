extends Node3D

@export var drag_start_helper: Node3D
@export var mouse_helper: Node3D # The 'cursor'
@export var view_camera: Camera3D # Used for raycasting mouse
@export var cash_display: Label
@export var wallgrid: WallGrid

@onready var buildable_buttons: Control = get_node("../CanvasLayer/Control/BuildableButtons")

var selected_structure: Structure = null
var cur_y: int = 0 # The current layer to interact with
var plane: Plane # Used for raycasting mouse
var drag_start: Vector3

func _ready():
	plane = Plane(Vector3.UP, Vector3.ZERO)

	var mesh_library = MeshLibrary.new()

	var game_config: GameConfig = ResourceLoader.load("res://game_config.tres")
	for structure in game_config.buildables:
		var id = mesh_library.get_last_unused_item_id()
		structure.id = id
		mesh_library.create_item(id)
		mesh_library.set_item_mesh(id, get_mesh(ResourceLoader.load(structure.model_file)))
		if structure.name == "wall":
			selected_structure = structure

	wallgrid.set_mesh_library(mesh_library)
	

	# Create buttons for each buildable
	for buildable in game_config.buildables:
		if buildable.placement_style == Globals.PlacementStyle.DO_NOT_PLACE:
			continue
		var button = Button.new()

		button.text = buildable.name

		button.connect("pressed", _on_buildable_button_pressed.bind(buildable))
		buildable_buttons.add_child(button)

	load_map()
	

func _unhandled_input(event: InputEvent) -> void:
	# Controls
	accept_actions()
	
	if event.is_action_pressed("layer up"):
		cur_y += 1
		if cur_y > 10:
			cur_y = 10
		plane.d = cur_y

	if event.is_action_pressed("layer down"):
		cur_y -= 1
		if cur_y < 0:
			cur_y = 0
		plane.d = cur_y
			
	if event.is_action_pressed("undo"):
		wallgrid.undo()
	
	# Map position based on mouse
	
	var world_position = plane.intersects_ray(
		view_camera.project_ray_origin(get_viewport().get_mouse_position()),
		view_camera.project_ray_normal(get_viewport().get_mouse_position()))

	if not world_position:
		return

	mouse_helper.position = world_position.round()

	wallgrid.update_visibility(world_position, view_camera.global_position)
	
	var build_id = selected_structure.id
	if event.is_action_pressed("negate_modifier"):
		build_id = -1  # HACK: flag value for deletion

	if event.is_action_pressed("build"):
		match selected_structure.placement_style:
			Globals.PlacementStyle.ROOM_PLOP:
				self.wallgrid.room_plop(world_position.round(), selected_structure.id)
			Globals.PlacementStyle.WALL_PLOP:
				print("TODO")
			_:
				drag_start_helper.position = world_position.round()
				drag_start_helper.show()
				drag_start = world_position

	if event.is_action_released("build"):
		drag_start_helper.hide()
		if selected_structure != null:
			match selected_structure.placement_style:
				Globals.PlacementStyle.WALL_DRAG:
					self.wallgrid.wall_drag(drag_start, world_position, selected_structure.id)
				Globals.PlacementStyle.FLOOR_DRAG:
					self.wallgrid.floor_drag(drag_start, world_position, selected_structure.id)


# Retrieve the mesh from a PackedScene, used for dynamically creating a MeshLibrary

func get_mesh(packed_scene: PackedScene):
	var scene_state: SceneState = packed_scene.get_state()
	for i in range(scene_state.get_node_count()):
		if (scene_state.get_node_type(i) == "MeshInstance3D"):
			for j in scene_state.get_node_property_count(i):
				var prop_name = scene_state.get_node_property_name(i, j)
				if prop_name == "mesh":
					var prop_value = scene_state.get_node_property_value(i, j)
					return prop_value.duplicate()
	assert(false)


# Saving/load

func load_map():
	print("Loading map...")
	#TODO

func accept_actions():
	if Input.is_action_pressed("save"):
		wallgrid.save()
	if Input.is_action_pressed("load"):
		wallgrid.load()

	if Input.is_action_pressed("quit"):
		get_tree().quit()

func _on_buildable_button_pressed(structure: Structure):
	selected_structure = structure
