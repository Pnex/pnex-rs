---
id: lab
title: Vision Lab — teach PNeX your own objects
kind: feature
pages: /lab, /lab/:id
nodes: vision_detect
err_codes: camera-unknown, camera-export-empty, ml-model-load-failed, ml-model-inference-failed, lab-project-not-found, lab-image-not-found, lab-version-not-found, lab-class-in-use, lab-annotations-conflict, lab-box-invalid, lab-asset-not-image, lab-dataset-empty, lab-video-unsupported, lab-video-too-large, lab-video-decode-failed, lab-interpolate-no-keyframes, lab-train-busy, lab-train-base-invalid, lab-train-no-data, lab-train-interrupted, lab-train-failed, lab-propagate-no-previous, lab-review-full
tools: 
tags: train, entraîner, fine-tune, affiner, epochs, époques, video, vidéo, mp4, mov, interpolate, interpoler, interpolation, recording, enregistrement, video frames, images de vidéo, pre-annotation, pré-annotation, suggest, proposer, vision lab, lab, dataset, annotation, annoter, boîte, bounding box, custom model, modèle personnalisé, reconnaissance d'objets, object recognition, training, entraînement, coco, yolox
---
Vision Lab (Media › Vision Lab) builds the dataset of a custom object detector: name the classes ("red_valve", "safety_helmet"…), add pictures from the media library, draw a box around every object, then freeze a dataset version and download it in COCO format.

## What you can do
- **New project**: a name and the list of classes. Classes can be added or renamed later; a class that still has boxes cannot be removed.
- **Add images**: pick pictures already uploaded in **Media**. Every import is one *source*: all its images go either to training or to validation, never split between both, so near-identical pictures never inflate the scores.
- **From a recording**: take one picture every N seconds from what a camera recorded (pick the camera, the day, the start and end times; at most one hour and 200 pictures per import). The pictures land in the media library and in the project as one source, so a whole video stays on the same side of training and validation. Only device cameras with a **Video recording** flow have recordings.
- **From a video**: pick a video of the media library (MP4 or MOV in H.264, H.265 or MJPEG; at most 512 MB), a start and an end in seconds and one picture every N seconds (at most one hour and 200 pictures per import). Every import of the same video belongs to one source. Phone videos work as recorded. Another format is refused with "not a readable video": convert it to MP4 (H.264) first.
- **Annotate**: select an image, pick a class (keys 1 to 9), drag a box around each object. Click a box to select it, **Delete** removes it. **No object in this image** marks a negative example (it teaches the model what is *not* the object). Every change is saved at once.
- **Pre-annotate**: pick a detection model of **Automation › Models** (for example the YOLOX COCO model) and **Suggest boxes**: the model proposes dashed boxes. A proposal whose label is a class name of the project (person, car, dog… or your own trained classes) takes that class; **Keep the matching proposals** keeps them all, a click keeps one (a grey proposal takes the current class), **Dismiss** drops the rest. Nothing is saved until you keep a box. Once a first custom model exists, use it here to annotate the next pictures faster.
- **Interpolate** (pictures from a video or a recording): annotate a picture, skip a few, annotate a later one, then on a picture in between click **Interpolate**: PNeX proposes the boxes moved in a straight line between the two annotated pictures of the same video, labelled "interpolated". Boxes are paired by class, nearest first; a box with no partner of its class on the other side is not proposed. Keep them like model proposals and correct the ones that drifted. It needs an annotated picture before and after in the same source.
- **Statistics**: boxes and images per class, train / validation counts, and warnings: fewer than 20 annotated images for a class, fewer than 5 validation images, no negative example. They are advice, never a blocker.
- **Freeze a version**: copies the annotated images and their boxes into a numbered, read-only dataset version; later edits never change it.
- **Export**: downloads a version as a zip (images + COCO annotations for train and validation), ready for a YOLOX training.

## Improvement loop
- **To review**: in an **Object detection** node, **Doubtful frames to the Vision Lab** picks a project; when the model hesitates (a score between **Min score** and **Max score**, 0.25 to 0.6 by default), the frame joins that project with the model's proposals, at most one per camera every 60 s. In the Lab those pictures carry a purple dot ("To review"), **Only the pictures to review** filters them, and opening one shows the proposals ready to keep. A picture leaves the queue once it has boxes or is marked without object. At most 500 pictures wait per project.
- **Propagate** (pictures from a video or a recording, a model chosen in the proposals bar): the model proposes the boxes of the picture, and each takes the class of the nearest box on the previous annotated picture of the same video. Useful when the model does not know your class names.
- **Diversity warnings**: besides the counts, the Lab warns when a class comes from fewer than 3 sources, always has the same size (always at the same distance) or the same place in the picture, and when all pictures have the same brightness. Add the missing kind of pictures, then train again.

## Training
In the **Training** panel: pick a frozen **Dataset version**, a **Starting model** (a YOLOX model of **Automation › Models**, for example the YOLOX COCO model) and a number of **Epochs** (30 by default), then **Train**. PNeX keeps the starting network and learns again only its last layers for your classes, on the server, without GPU: a few minutes for a few hundred pictures. The run shows its progress (reading the pictures, epochs with the loss, evaluation) and, once done, the **validation mAP@0.5** (how well it finds the objects of the validation pictures, 100 % = all found with no false alarm). The new model joins **Automation › Models** with your class names, already checked: pick it in an **Object detection** node.

- One training per project at a time ("already queued or running"); **Cancel** stops it.
- No validation picture in the version = no mAP: give each class pictures from several imports or force some to validation.
- A run stopped by a server restart shows "the training stopped": start it again.
- Objects very different from everyday ones (the starting model knows people, vehicles, animals, objects of daily life) are learned less well with this method: add varied pictures, more epochs, and compare the mAP of the versions.
- The exported COCO zip stays available to train elsewhere with any YOLOX tool; the ONNX is registered in **Automation › Models** the same way.

## Good to know
- Annotate the object in many situations: angles, distances, light, partly hidden. Twenty varied pictures beat a hundred identical ones.
- A box should hug the visible part of the object, including when it touches the image border.
- Two people editing the same image at the same time: the second save is refused ("changed meanwhile"); reload the image and redo the change.
- Only Owners, Admins and Members change a project; Viewers can browse and export.
