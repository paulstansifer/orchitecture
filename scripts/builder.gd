extends Node3D

@export var drag_start_helper: Node3D
@export var mouse_helper: Node3D # The 'cursor'
@export var mouse_helper_room: Node3D # The 'cursor' for rooms
@export var view_camera: Camera3D # Used for raycasting mouse
@export var wallgrid: WallGrid

@export var buildable_buttons: Control
@export var filename: TextEdit
@export var save_button: Button
@export var load_button: Button
@export var evaluation: RichTextLabel
@export var load_options: OptionButton

var cur_dir: int = 0
var selected_structure_idx: int = 0
var cur_y: int = 0 # The current layer to interact with
var plane: Plane # Used for raycasting mouse
var drag_start: Vector3

func _ready():
	plane = Plane(Vector3.UP, Vector3.ZERO)

	var mesh_library = MeshLibrary.new()

	
	var structures = wallgrid.get_structures()

	# Create buttons for each buildable
	for idx in structures.size():
		var button = Button.new()

		button.text = structures[idx]

		button.connect("pressed", _on_buildable_button_pressed.bind(idx))
		buildable_buttons.add_child(button)
	
	for file in DirAccess.open("res://training").get_files():
		load_options.add_item(file)
	for i in range(load_options.item_count):
		load_options.get_popup().set_item_as_radio_checkable(i, false)
		
	save_button.connect("pressed", save_command)
	load_button.connect("pressed", load_command)
	load_options.connect("item_selected", select_command)

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

	if event.is_action_pressed("rotate_object"):
		cur_dir = (cur_dir + 3) % 4
		
			
	if event.is_action_pressed("undo"):
		wallgrid.undo()
	
	# Map position based on mouse
	
	var world_position = plane.intersects_ray(
		view_camera.project_ray_origin(get_viewport().get_mouse_position()),
		view_camera.project_ray_normal(get_viewport().get_mouse_position()))
		
	
	if event.is_action_pressed("evaluate"):
		evaluation.clear()
		evaluation.add_text("working...")
		var metrics = wallgrid.metrics_at(world_position)
		evaluation.clear()
		evaluation.add_text("coherence: %.2f  interest: %.2f" % metrics)

	if not world_position:
		return

	if wallgrid.structure_is_room_plop(selected_structure_idx):
		mouse_helper.hide()
		mouse_helper_room.show()
		mouse_helper_room.rotation = Vector3(0, 0, 0)
		mouse_helper_room.rotate_y(cur_dir * TAU / 4)
		mouse_helper_room.position = world_position.round() + Vector3(0.5, 0, 0.5)
	else:
		mouse_helper_room.hide()
		mouse_helper.show()
		mouse_helper.position = world_position.round()

	wallgrid.update_visibility(world_position, view_camera.global_position)
	
	var remove = false
	if Input.is_action_pressed("negate_modifier"):
		remove = true

	if event.is_action_pressed("build"):
		self.wallgrid.click(world_position, selected_structure_idx, cur_dir, remove)

		drag_start_helper.position = world_position.round()
		drag_start_helper.show()
		drag_start = world_position

	if event.is_action_released("build"):
		drag_start_helper.hide()

		self.wallgrid.drag(drag_start, world_position, selected_structure_idx, remove)


# Retrieve the mesh from a PackedScene, used for dynamically creating a MeshLibrary


# Saving/load

func save_command():
	wallgrid.save(filename.text)
	var content = FileAccess.get_file_as_bytes(filename.text)
	JavaScriptBridge.download_buffer(content, filename.text)

func load_command():
	wallgrid.load(filename.text)

func select_command(index):
	print(load_options.get_item_text(index))
	wallgrid.load(load_options.get_item_text(index))

func accept_actions():
	if Input.is_action_pressed("quit"):
		wallgrid.get_ready_to_quit()
		get_tree().quit()

func _on_buildable_button_pressed(idx: int):
	selected_structure_idx = idx
